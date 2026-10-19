//! Command handlers. Each takes the decoded request and a [`Ctx`] and
//! returns the HTTP response.

mod estimate;
mod folder_sync;
mod provision;
mod sync;

use crate::app::AppState;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use eas_jmap::{JmapClient, JmapError};
use eas_proto::{Command, ProtocolVersion, RequestLine};
use eas_state::{Device, StoreError};
use eas_wbxml::Element;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

/// Everything a handler needs about the request it serves.
pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub jmap: JmapClient,
    pub device: Device,
    pub line: &'a RequestLine,
    /// Negotiated version; read once 16.x-only behaviour (drafts, Find,
    /// calendar instance edits) lands.
    #[allow(dead_code)]
    pub version: ProtocolVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error(transparent)]
    Jmap(#[from] JmapError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("malformed request: {0}")]
    BadRequest(&'static str),
}

impl IntoResponse for HandlerError {
    fn into_response(self) -> Response {
        match self {
            HandlerError::Jmap(e) => crate::app::jmap_error(e),
            HandlerError::Store(e) => {
                tracing::error!(error = %e, "state store failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(header::RETRY_AFTER, "60")],
                )
                    .into_response()
            }
            HandlerError::BadRequest(what) => (StatusCode::BAD_REQUEST, what).into_response(),
        }
    }
}

pub type HandlerResult = Result<Response, HandlerError>;

/// A WBXML response body.
pub fn wbxml(el: &Element) -> Response {
    tracing::debug!(body = el.to_xml(), "EAS response");
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(eas_wbxml::CONTENT_TYPE),
        )],
        eas_wbxml::encode(el),
    )
        .into_response()
}

/// An empty 200, which `Sync` uses for "nothing changed".
pub fn empty() -> Response {
    StatusCode::OK.into_response()
}

pub async fn dispatch(ctx: Ctx<'_>, command: Command, request: Option<Element>) -> Response {
    let result = match command {
        Command::Provision => provision::handle(&ctx, request).await,
        Command::FolderSync => folder_sync::handle(&ctx, request).await,
        Command::Sync => sync::handle(&ctx, request).await,
        Command::GetItemEstimate => estimate::handle(&ctx, request).await,
        other => {
            tracing::info!(user = %ctx.device.username, device = %ctx.line.device_id, command = %other, "command not implemented yet");
            Ok((
                StatusCode::NOT_IMPLEMENTED,
                format!("{other} is not implemented yet"),
            )
                .into_response())
        }
    };
    result.unwrap_or_else(IntoResponse::into_response)
}

/// A random, non-zero `u32`, for policy keys and SyncKey suffixes.
pub fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    loop {
        if getrandom::getrandom(&mut b).is_err() {
            // No OS randomness: fall back to the clock, still unique enough
            // for a key that only has to differ from the previous one.
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
                .unwrap_or(1);
            return t.max(1);
        }
        let v = u32::from_le_bytes(b);
        if v != 0 {
            return v;
        }
    }
}

/// A SyncKey: the counter keeps keys ordered for humans reading logs, the
/// random part keeps a key from a lost database from ever matching a new one.
pub fn sync_key(counter: i64) -> String {
    format!("{counter}-{:08x}", random_u32())
}

/// One lock per device, so a retry that overlaps the original request can't
/// interleave two state updates.
#[derive(Default)]
pub struct DeviceLocks {
    map: Mutex<HashMap<i64, Weak<tokio::sync::Mutex<()>>>>,
}

impl DeviceLocks {
    pub fn get(&self, device: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = map.get(&device).and_then(Weak::upgrade) {
            return l;
        }
        if map.len() > 1_000 {
            map.retain(|_, w| w.strong_count() > 0);
        }
        let l = Arc::new(tokio::sync::Mutex::new(()));
        map.insert(device, Arc::downgrade(&l));
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_keys_differ_and_fit() {
        let a = sync_key(1);
        let b = sync_key(1);
        assert_ne!(a, b);
        assert!(a.starts_with("1-") && a.len() <= 64);
    }

    #[tokio::test]
    async fn device_locks_are_shared() {
        let locks = DeviceLocks::default();
        let a = locks.get(1);
        let _g = a.lock().await;
        assert!(locks.get(1).try_lock().is_err());
        assert!(locks.get(2).try_lock().is_ok());
    }
}
