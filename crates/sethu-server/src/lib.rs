//! Sethu server library. The binary is a thin wrapper around [`app`].

pub mod adb;
pub mod api;
pub mod assets;
pub mod auth;
pub mod config;
pub mod error;
pub mod files;
pub mod pairing;
pub mod rate_limit;
pub mod security;
pub mod state;
pub mod store;
pub mod target;
pub mod transfer;
pub mod util;
pub mod wda;
pub mod web;
pub mod ws;

use axum::{middleware, Router};
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::state::AppState;

/// Build the complete HTTP application (API, WebSockets and static frontend).
pub fn app(state: AppState) -> Router {
    Router::new()
        .merge(api::router())
        .merge(files::router())
        .merge(pairing::router())
        .merge(transfer::router())
        .merge(ws::router())
        .fallback(web::serve)
        .with_state(state.clone())
        // Applied to everything: security headers, then origin/CSRF checks.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::hardening_headers,
        ))
        .layer(middleware::from_fn_with_state(state, security::guard))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}
