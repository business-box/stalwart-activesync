//! Admin API: devices and remote wipe, behind a bearer token.
//!
//! Enabled only when `EAS_ADMIN_TOKEN` is set. Keep `/admin` off the public
//! reverse proxy; the token is a second line of defence, not the first.
//!
//! | Method and path | Effect |
//! | --- | --- |
//! | `GET /admin/devices?user=` | List devices, optionally one user's |
//! | `POST /admin/devices/{id}/wipe` | Ask the device to wipe itself |
//! | `POST /admin/devices/{id}/block`, `/unblock` | Refuse or allow the device |
//! | `POST /admin/devices/{id}/reset` | Make it provision and resync from scratch |
//! | `DELETE /admin/devices/{id}` | Forget the device (also cancels a wipe) |

use crate::app::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/devices", get(list))
        .route("/admin/devices/{id}", axum::routing::delete(delete))
        .route("/admin/devices/{id}/{action}", post(action))
}

/// Constant-time token check (hashing first evens out lengths).
fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.config.admin_token.as_deref() else {
        return false;
    };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.trim().as_bytes());
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if state.config.admin_token.is_none() {
        return Some(StatusCode::NOT_FOUND.into_response());
    }
    (!authorized(state, headers)).then(|| StatusCode::UNAUTHORIZED.into_response())
}

fn store_error(e: eas_state::StoreError) -> Response {
    tracing::error!(error = %e, "admin: state store failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    match state
        .store
        .list_devices(q.get("user").map(String::as_str))
        .await
    {
        Ok(devices) => Json(
            devices
                .into_iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "user": d.username,
                        "deviceId": d.device_id,
                        "deviceType": d.device_type,
                        "userAgent": d.user_agent,
                        "protocolVersion": d.protocol_version,
                        "model": d.model,
                        "friendlyName": d.friendly_name,
                        "os": d.os,
                        "provisioned": d.provisioned,
                        "wipe": d.wipe.as_str(),
                        "blocked": d.blocked,
                        "firstSeen": d.first_seen,
                        "lastSeen": d.last_seen,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => store_error(e),
    }
}

async fn action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, action)): Path<(i64, String)>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let store = &state.store;
    let result = match action.as_str() {
        "wipe" => store.request_wipe(id).await,
        "block" => store.set_blocked(id, true).await,
        "unblock" => store.set_blocked(id, false).await,
        "reset" => store.reset_device(id).await,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match result {
        Ok(true) => {
            tracing::warn!(device = id, %action, "admin action");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => store_error(e),
    }
}

async fn delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    match state.store.delete_device(id).await {
        Ok(true) => {
            tracing::warn!(device = id, "admin: device deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => store_error(e),
    }
}
