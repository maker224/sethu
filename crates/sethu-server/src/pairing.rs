//! QR pairing: an authenticated browser mints a short-lived, single-use token.
//! Scanning the QR opens `/pair/<token>`, which creates a session and redirects
//! the phone or tablet into the same workspace.

use std::{
    collections::HashMap,
    net::{IpAddr, UdpSocket},
    sync::Mutex,
};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::{self, AuthUser},
    error::AppError,
    state::AppState,
    util::{hex, now_unix, random_bytes},
};

#[derive(Clone)]
struct Pairing {
    user_id: Uuid,
    host_id: Option<Uuid>,
    expires_at: i64,
    redeemed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairStatus {
    Pending,
    Redeemed,
    Expired,
}

pub struct PairingStore {
    inner: Mutex<HashMap<String, Pairing>>,
}

impl PairingStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn create(&self, user_id: Uuid, host_id: Option<Uuid>, ttl: std::time::Duration) -> String {
        let token = hex(&random_bytes::<24>());
        let now = now_unix();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, pairing| pairing.expires_at > now);
        map.insert(
            token.clone(),
            Pairing {
                user_id,
                host_id,
                expires_at: now + ttl.as_secs() as i64,
                redeemed: false,
            },
        );
        token
    }

    fn take(&self, token: &str) -> Option<Pairing> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_unix();
        if map
            .get(token)
            .is_some_and(|pairing| pairing.expires_at <= now)
        {
            map.remove(token);
            return None;
        }
        let pairing = map.get_mut(token)?;
        if pairing.redeemed {
            return None;
        }
        pairing.redeemed = true;
        Some(pairing.clone())
    }

    /// Read without consuming, so link previews can fetch the confirmation page.
    fn get(&self, token: &str) -> Option<Pairing> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pairing = map.get(token)?.clone();
        if pairing.expires_at <= now_unix() {
            map.remove(token);
            return None;
        }
        Some(pairing)
    }

    fn status(&self, token: &str, owner: Uuid) -> Option<PairStatus> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pairing = map.get(token)?.clone();
        if pairing.user_id != owner {
            return None;
        }
        if pairing.expires_at <= now_unix() {
            map.remove(token);
            return Some(PairStatus::Expired);
        }
        Some(if pairing.redeemed {
            PairStatus::Redeemed
        } else {
            PairStatus::Pending
        })
    }
}

impl Default for PairingStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/pair", post(create_pair))
        .route("/api/pair/{token}", get(pair_status))
        .route("/pair/{token}", get(pair_page).post(redeem_pair))
}

#[derive(Deserialize)]
struct PairRequest {
    #[serde(default)]
    host_id: Option<Uuid>,
}

#[derive(Serialize)]
struct PairResponse {
    url: String,
    /// `data:image/svg+xml;base64,...` for an `<img>` tag.
    qr: String,
    expires_in: u64,
    target: String,
    /// Lets the desktop poll whether the phone has completed pairing.
    token: String,
}

#[derive(Serialize)]
struct PairStatusView {
    status: &'static str,
}

async fn create_pair(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Result<Json<PairResponse>, AppError> {
    let host = match body.host_id {
        Some(id) => Some(
            state
                .store
                .host(id, user.user.id)
                .ok_or_else(|| AppError::NotFound("device not found".into()))?,
        ),
        None => None,
    };

    let token = state.pairings.create(
        user.user.id,
        host.as_ref().map(|host| host.id),
        state.cfg.pair_ttl,
    );
    let url = format!("{}/pair/{token}", base_url(&state, &headers));
    let qr = qr_data_uri(&url)?;
    tracing::info!(
        user_id = %user.user.id,
        host_id = ?host.as_ref().map(|host| host.id),
        "pairing code created"
    );

    Ok(Json(PairResponse {
        url,
        qr,
        expires_in: state.cfg.pair_ttl.as_secs(),
        target: host
            .map(|host| host.name)
            .unwrap_or_else(|| "this workspace".into()),
        token,
    }))
}

async fn pair_status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(token): Path<String>,
) -> Result<Json<PairStatusView>, AppError> {
    let status = state
        .pairings
        .status(&token, user.user.id)
        .ok_or_else(|| AppError::NotFound("pairing code not found".into()))?;
    Ok(Json(PairStatusView {
        status: match status {
            PairStatus::Pending => "pending",
            PairStatus::Redeemed => "redeemed",
            PairStatus::Expired => "expired",
        },
    }))
}

async fn pair_page(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(pairing) = state.pairings.get(&token) else {
        tracing::warn!("pairing token rejected");
        return pairing_error("This pairing link is invalid or has expired.");
    };
    if pairing.redeemed {
        return pairing_error("This pairing link has already been used.");
    }

    let host = pairing
        .host_id
        .and_then(|id| state.store.host(id, pairing.user_id));
    let target = match &host {
        Some(host) => format!("Open “{}” on this device", html_escape(&host.name)),
        None => "Sign in to this workspace".to_string(),
    };
    let user = state
        .store
        .user_by_id(pairing.user_id)
        .map(|user| html_escape(&user.email))
        .unwrap_or_else(|| "this workspace".into());

    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"referrer\" content=\"no-referrer\"><title>Connect to Sethu</title></head>\
         <body style=\"margin:0;min-height:100vh;display:grid;place-items:center;background:#05070d;color:#e9eefb;font-family:-apple-system,system-ui,sans-serif\">\
         <main style=\"max-width:420px;padding:28px;text-align:center\">\
         <h1 style=\"font-size:1.25rem;letter-spacing:-.02em\">{target}</h1>\
         <p style=\"color:#94a3b8;line-height:1.6\">Signed in as {user}. Only continue if you scanned this code yourself.</p>\
         <form id=\"pair-form\" method=\"post\" action=\"/pair/{token}\">\
         <button type=\"submit\" style=\"margin-top:8px;padding:12px 20px;border:0;border-radius:12px;background:linear-gradient(135deg,#5eead4,#7dd3fc 55%,#a5b4fc);color:#04121a;font:inherit;font-weight:700;cursor:pointer\">Connect this device</button>\
         </form>\
         <p style=\"margin-top:18px\"><a style=\"color:#5eead4\" href=\"/\">Cancel</a></p>\
         <script src=\"/pair.js\" defer></script>\
         </main></body></html>"
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn redeem_pair(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(pairing) = state.pairings.take(&token) else {
        tracing::warn!("pairing token rejected on redemption");
        return pairing_error("This pairing link is invalid or has expired.");
    };

    let session = match state
        .store
        .create_session(pairing.user_id, state.cfg.session_ttl)
    {
        Ok((token, session)) => (token, session),
        Err(err) => {
            tracing::error!(error = %err, "pairing session creation failed");
            return pairing_error("Could not create a session for this pairing link.");
        }
    };

    let location = pairing
        .host_id
        .map(|id| format!("/viewer.html?host={id}"))
        .unwrap_or_else(|| "/".to_string());
    let mut response = Redirect::to(&location).into_response();
    if let Ok(value) = HeaderValue::from_str(&auth::session_cookie(&state, &session.0)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    tracing::info!(user_id = %session.1.user_id, "pairing completed");
    response
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn pairing_error(message: &str) -> Response {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Sethu pairing</title></head>\
         <body style=\"margin:0;min-height:100vh;display:grid;place-items:center;background:#05070d;color:#e9eefb;font-family:-apple-system,system-ui,sans-serif\">\
         <main style=\"max-width:420px;padding:28px;text-align:center\">\
         <h1 style=\"font-size:1.2rem\">Pairing failed</h1><p style=\"color:#94a3b8;line-height:1.6\">{message}</p>\
         <p><a style=\"color:#5eead4\" href=\"/\">Back to Sethu</a></p></main></body></html>"
    );
    (
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "no-store")],
        Html(body),
    )
        .into_response()
}

fn qr_data_uri(url: &str) -> Result<String, AppError> {
    let code = QrCode::new(url.as_bytes())
        .map_err(|err| AppError::internal(format!("qr encode: {err}")))?;
    let svg = code.render::<svg::Color>().min_dimensions(240, 240).build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(public) = &state.cfg.public_url {
        return public.clone();
    }

    let forwarded_proto = if state.cfg.trust_proxy {
        headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
    } else {
        None
    };
    let scheme = match forwarded_proto {
        Some("https") => "https",
        Some("http") => "http",
        _ if state.cfg.cookie_secure => "https",
        _ => "http",
    };

    let forwarded_host = if state.cfg.trust_proxy {
        headers
            .get("x-forwarded-host")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let host = forwarded_host
        .map(str::to_string)
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "localhost:8080".into());

    format!("{scheme}://{}", localize_host(&host))
}

/// If the desktop opened Sethu on localhost, a phone cannot scan that URL.
/// Substitute the host's LAN IP when one can be determined.
fn localize_host(host: &str) -> String {
    let (name, port) = split_host_port(host);
    let loopback = name.eq_ignore_ascii_case("localhost")
        || name == "127.0.0.1"
        || name == "[::1]"
        || name == "::1";
    if !loopback {
        return host.to_string();
    }
    let Some(ip) = lan_ip() else {
        return host.to_string();
    };
    match port {
        Some(port) => format!("{ip}:{port}"),
        None => ip.to_string(),
    }
}

fn split_host_port(host: &str) -> (&str, Option<&str>) {
    if host.starts_with('[') {
        if let Some(end) = host.find(']') {
            let name = &host[..=end];
            let port = host[end + 1..].strip_prefix(':');
            return (name, port);
        }
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => (name, Some(port)),
        _ => (host, None),
    }
}

fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts_keep_ports() {
        assert_eq!(
            split_host_port("localhost:8080"),
            ("localhost", Some("8080"))
        );
        assert_eq!(split_host_port("[::1]:9000"), ("[::1]", Some("9000")));
        assert_eq!(split_host_port("example.com"), ("example.com", None));
    }

    #[test]
    fn qr_data_uri_is_an_svg_image() {
        let uri = qr_data_uri("http://192.168.1.2:8080/pair/abc").unwrap();
        assert!(uri.starts_with("data:image/svg+xml;base64,"));
        assert!(uri.len() > 500);
    }
}
