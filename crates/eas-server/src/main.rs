//! `stalwart-activesync`: Exchange ActiveSync gateway for Stalwart.
//!
//! Configuration comes from the environment; see `config.rs` and the README.

mod admin;
mod app;
mod auth;
mod config;
mod handlers;
mod load;
mod metrics;
mod probe;
mod push;

#[cfg(test)]
mod e2e_tests;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,stalwart_activesync=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("probe") => {
            if let Err(e) = probe::run(&args[1..]).await {
                eprintln!("probe failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("load") => {
            if let Err(e) = load::run(&args[1..]).await {
                eprintln!("load test failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }

    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };
    let endpoint = match eas_jmap::StalwartEndpoint::new(&config.stalwart_url) {
        Ok(e) => e.with_url_rewrite(config.rewrite_urls),
        Err(e) => {
            eprintln!("configuration error: STALWART_URL: {e}");
            std::process::exit(2);
        }
    };
    let auth = auth::Authenticator::new(endpoint, Duration::from_secs(config.auth_cache_secs));
    let store = match eas_state::Store::open(&config.db_url).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open {}: {e}", config.db_url);
            std::process::exit(1);
        }
    };
    let listen = config.listen;
    let state = Arc::new(app::AppState {
        config,
        auth,
        store,
        limits: eas_jmap::UserLimits::new(),
        locks: handlers::DeviceLocks::default(),
        push: push::PushHub::default(),
        pings: push::PingSlots::default(),
        metrics: metrics::Metrics::default(),
        policy: eas_proto::provision::Policy::default(),
    });

    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!(%listen, "stalwart-activesync listening");
    let app = app::router(state).into_make_service_with_connect_info::<SocketAddr>();
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_app() -> axum::Router {
        let config = config::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            // Nothing listens here; tests below never reach Stalwart.
            stalwart_url: "http://127.0.0.1:9/".into(),
            public_url: "https://mail.example.org".into(),
            trust_forwarded: true,
            max_body_bytes: 1024 * 1024,
            auth_cache_secs: 300,
            db_url: "sqlite::memory:".into(),
            require_provisioning: true,
            admin_token: Some("admintoken".into()),
            rewrite_urls: true,
        };
        let endpoint = eas_jmap::StalwartEndpoint::new(&config.stalwart_url).unwrap();
        let auth = auth::Authenticator::new(endpoint, Duration::from_secs(300));
        let store = eas_state::Store::memory().await.unwrap();
        app::router(Arc::new(app::AppState {
            config,
            auth,
            store,
            limits: eas_jmap::UserLimits::new(),
            locks: handlers::DeviceLocks::default(),
            push: push::PushHub::default(),
            pings: push::PingSlots::default(),
            metrics: metrics::Metrics::default(),
            policy: eas_proto::provision::Policy::default(),
        }))
        .layer(axum::extract::connect_info::MockConnectInfo(
            SocketAddr::from(([127, 0, 0, 1], 40000)),
        ))
    }

    #[tokio::test]
    async fn options_lists_versions() {
        let resp = test_app()
            .await
            .oneshot(
                Request::options("/Microsoft-Server-ActiveSync?User=a&DeviceId=x&DeviceType=y")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()["ms-asprotocolversions"],
            "14.0,14.1,16.0,16.1"
        );
        assert!(resp.headers()["ms-asprotocolcommands"]
            .to_str()
            .unwrap()
            .contains("FolderSync"));
    }

    #[tokio::test]
    async fn post_without_credentials_is_401() {
        let resp = test_app()
            .await
            .oneshot(
                Request::post(
                    "/Microsoft-Server-ActiveSync?Cmd=FolderSync&User=a&DeviceId=x&DeviceType=y",
                )
                .header("MS-ASProtocolVersion", "16.1")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key("www-authenticate"));
    }

    #[tokio::test]
    async fn bad_request_lines_are_400() {
        for uri in [
            "/Microsoft-Server-ActiveSync?Cmd=Bogus&DeviceId=x&DeviceType=y",
            "/Microsoft-Server-ActiveSync?User=a&DeviceId=x&DeviceType=y",
            "/Microsoft-Server-ActiveSync?Cmd=Sync&DeviceId=x%27&DeviceType=y",
        ] {
            let resp = test_app()
                .await
                .oneshot(Request::post(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
        let resp = test_app()
            .await
            .oneshot(
                Request::post(
                    "/Microsoft-Server-ActiveSync?Cmd=Sync&User=a&DeviceId=x&DeviceType=y",
                )
                .header("MS-ASProtocolVersion", "12.1")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unreachable_stalwart_is_503_with_retry_after() {
        let resp = test_app()
            .await
            .oneshot(
                Request::post(
                    "/Microsoft-Server-ActiveSync?Cmd=FolderSync&User=a&DeviceId=x&DeviceType=y",
                )
                .header("MS-ASProtocolVersion", "16.1")
                .header("Authorization", "Basic YTpi")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(resp.headers().contains_key("retry-after"));
    }

    #[tokio::test]
    async fn autodiscover_requires_auth_and_rejects_garbage() {
        let body = r#"<Autodiscover><Request><EMailAddress>a@example.org</EMailAddress><AcceptableResponseSchema>http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006</AcceptableResponseSchema></Request></Autodiscover>"#;
        let resp = test_app()
            .await
            .oneshot(
                Request::post("/autodiscover/autodiscover.xml")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = test_app()
            .await
            .oneshot(
                Request::post("/Autodiscover/Autodiscover.xml")
                    .body(Body::from("nope"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let text = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&text[..], b"invalid autodiscover request");
    }
}
