use crate::auth::{basic_credentials, Authenticator};
use crate::config::Config;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use eas_jmap::{JmapError, Session};
use eas_proto::autodiscover::{mobilesync_response, AutodiscoverRequest};
use eas_proto::{options, Command, ProtocolVersion, RequestLine};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

pub struct AppState {
    pub config: Config,
    pub auth: Authenticator,
}

pub fn router(state: Arc<AppState>) -> Router {
    let max_body = state.config.max_body_bytes;
    Router::new()
        .route(
            eas_proto::ENDPOINT_PATH,
            post(activesync_post).options(activesync_options),
        )
        .route("/autodiscover/autodiscover.xml", post(autodiscover))
        .route("/Autodiscover/Autodiscover.xml", post(autodiscover))
        .route("/AutoDiscover/AutoDiscover.xml", post(autodiscover))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state)
}

/// The phone's address: the first `X-Forwarded-For` entry when the gateway
/// sits behind Caddy, else the TCP peer.
fn client_ip(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Option<IpAddr> {
    if state.config.trust_forwarded {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return Some(ip);
        }
    }
    Some(peer.ip())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"ActiveSync\"")],
    )
        .into_response()
}

fn jmap_error(err: JmapError) -> Response {
    match err {
        JmapError::Unauthorized => unauthorized(),
        JmapError::Throttled => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "300")],
        )
            .into_response(),
        other => {
            tracing::warn!(error = %other, "Stalwart unavailable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "60")],
            )
                .into_response()
        }
    }
}

async fn activesync_options() -> Response {
    let mut headers = HeaderMap::new();
    for (name, value) in options::headers() {
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            headers.insert(n, v);
        }
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    (StatusCode::OK, headers).into_response()
}

async fn activesync_post(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let line = match RequestLine::parse(query.as_deref().unwrap_or("")) {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!(error = %e, "bad request line");
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    let command = match line.command() {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let version = match line.protocol_version {
        Some(v) => v,
        None => match headers
            .get("ms-asprotocolversion")
            .and_then(|v| v.to_str().ok())
            .map(str::parse::<ProtocolVersion>)
        {
            Some(Ok(v)) => v,
            Some(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
            // Old clients omit the header; they get the oldest version we speak.
            None => ProtocolVersion::V14_0,
        },
    };

    let Some(creds) = basic_credentials(&headers) else {
        return unauthorized();
    };
    let ip = client_ip(&state, &headers, peer);
    let session = match state.auth.authenticate(&creds, ip).await {
        Ok(s) => s,
        Err(e) => return jmap_error(e),
    };

    let request = if body.is_empty() {
        None
    } else {
        match eas_wbxml::decode(&body) {
            Ok(el) => Some(el),
            Err(e) => {
                tracing::info!(device = %line.device_id, %command, error = %e, "undecodable WBXML body");
                return (StatusCode::BAD_REQUEST, "invalid WBXML").into_response();
            }
        }
    };

    tracing::debug!(
        user = %session.username,
        device = %line.device_id,
        device_type = %line.device_type,
        %command,
        %version,
        body = request.as_ref().map(|r| r.to_xml()).unwrap_or_default(),
        "EAS request"
    );

    dispatch(&state, &session, &line, command, version, request).await
}

/// Command dispatch. Handlers arrive phase by phase (docs/PLAN.md); until a
/// command is implemented the gateway answers 501 so clients fail loudly
/// instead of syncing wrong data.
async fn dispatch(
    _state: &AppState,
    session: &Session,
    line: &RequestLine,
    command: Command,
    _version: ProtocolVersion,
    _request: Option<eas_wbxml::Element>,
) -> Response {
    tracing::info!(user = %session.username, device = %line.device_id, %command, "command not implemented yet");
    (
        StatusCode::NOT_IMPLEMENTED,
        format!("{command} is not implemented yet"),
    )
        .into_response()
}

async fn autodiscover(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let text = String::from_utf8_lossy(&body);
    let Some(req) = AutodiscoverRequest::parse(&text) else {
        return (StatusCode::BAD_REQUEST, "invalid autodiscover request").into_response();
    };
    if !req.is_mobilesync() {
        return proxy_autodiscover(&state, &headers, body).await;
    }
    // Like Exchange, only answer authenticated requests; this also stops the
    // endpoint from confirming which addresses exist.
    let Some(creds) = basic_credentials(&headers) else {
        return unauthorized();
    };
    let ip = client_ip(&state, &headers, peer);
    if let Err(e) = state.auth.authenticate(&creds, ip).await {
        return jmap_error(e);
    }
    let xml = mobilesync_response(&req.email, &req.email, &state.config.eas_url());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// Outlook-schema requests belong to Stalwart; pass them through unchanged.
async fn proxy_autodiscover(state: &AppState, headers: &HeaderMap, body: Bytes) -> Response {
    let endpoint = state.auth.endpoint();
    let Ok(url) = endpoint.base_url().join("autodiscover/autodiscover.xml") else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut req = endpoint.http().post(url).body(body);
    for name in [header::CONTENT_TYPE, header::AUTHORIZATION] {
        if let Some(v) = headers.get(&name) {
            req = req.header(name.as_str(), v.as_bytes());
        }
    }
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ctype = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/xml")
                .to_string();
            match resp.bytes().await {
                Ok(b) => (status, [(header::CONTENT_TYPE, ctype)], b.to_vec()).into_response(),
                Err(_) => StatusCode::BAD_GATEWAY.into_response(),
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "autodiscover passthrough failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
