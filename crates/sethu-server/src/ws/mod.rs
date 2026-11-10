//! WebSocket bridges: raw VNC passthrough for macOS and a screenshot/command
//! bridge for iPhone.

mod android;
mod ios;
mod vnc;

use axum::{routing::get, Router};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::AppError,
    state::AppState,
    store::{Host, HostKind},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ws/vnc/{id}", get(vnc::upgrade))
        .route("/ws/ios/{id}", get(ios::upgrade))
        .route("/ws/android/{id}", get(android::upgrade))
}

/// Load a host owned by the caller and make sure its kind matches the endpoint.
async fn host_for(
    state: &AppState,
    user: &AuthUser,
    id: Uuid,
    allowed: &[HostKind],
) -> Result<Host, AppError> {
    let host = state
        .store
        .host(id, user.user.id)
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    if !allowed.contains(&host.kind) {
        return Err(AppError::BadRequest(
            "this device cannot be opened with that viewer".into(),
        ));
    }
    Ok(host)
}
