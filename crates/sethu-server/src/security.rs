//! Cross-cutting HTTP security: origin checks, CSRF enforcement, login rate
//! limiting and hardening response headers.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, Method},
    middleware::Next,
    response::Response,
};

use crate::{
    auth::{self, CSRF_HEADER},
    error::AppError,
    rate_limit::RateLimiter,
    state::AppState,
};

const CSP: &str = "default-src 'self'; \
script-src 'self' https://cdn.jsdelivr.net; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; \
connect-src 'self' ws: wss: https://cdn.jsdelivr.net; \
font-src 'self' data:; \
frame-ancestors 'none'; \
base-uri 'none'; \
form-action 'self'; \
object-src 'none'";

/// Guard applied to `/api` and `/ws` traffic.
pub async fn guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = req.uri().path();
    let method = req.method().clone();
    let ip = client_ip(
        &state,
        req.headers(),
        req.extensions().get::<ConnectInfo<SocketAddr>>(),
    );

    if path.starts_with("/api") || path.starts_with("/ws") {
        enforce_same_origin(&state, &req)?;
    }

    if path.starts_with("/api") && is_unsafe(&method) {
        match path {
            // Unauthenticated endpoints: throttle instead of CSRF.
            "/api/auth/login" => {
                if !state
                    .limiter
                    .check(&RateLimiter::key(ip), 20, Duration::from_secs(300))
                {
                    tracing::warn!(?ip, "login rate limit hit");
                    return Err(AppError::TooManyRequests);
                }
            }
            "/api/auth/register" => {
                if !state
                    .limiter
                    .check(&RateLimiter::key(ip), 10, Duration::from_secs(3_600))
                {
                    tracing::warn!(?ip, "registration rate limit hit");
                    return Err(AppError::TooManyRequests);
                }
            }
            _ => {
                if !state.cfg.open_access {
                    let provided = req
                        .headers()
                        .get(CSRF_HEADER)
                        .and_then(|value| value.to_str().ok());
                    auth::require_csrf(&state, req.headers(), provided)?;
                }
            }
        }
    }

    Ok(next.run(req).await)
}

/// Adds headers that are cheap insurance for a remote-access UI.
pub async fn hardening_headers(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    let mut set = |name: header::HeaderName, value: &'static str| {
        headers.insert(name, value.parse().expect("static header value"));
    };

    set(header::CONTENT_SECURITY_POLICY, CSP);
    set(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(header::X_FRAME_OPTIONS, "DENY");
    set(header::REFERRER_POLICY, "no-referrer");
    set(
        header::HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=(), payment=()",
    );
    set(
        header::HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    if state.cfg.cookie_secure {
        set(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        );
    }
    response
}

fn is_unsafe(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Browsers send `Origin` on WebSocket upgrades and cross-origin fetch calls.
/// Requiring it to match the request `Host` (or an explicitly allowed origin)
/// blocks drive-by cross-site access even before SameSite cookies apply.
fn enforce_same_origin(state: &AppState, req: &Request) -> Result<(), AppError> {
    let Some(origin) = req.headers().get(header::ORIGIN) else {
        // Non-browser clients (curl, tests) do not send Origin. CSRF tokens are
        // still required for mutations, so this is safe.
        return Ok(());
    };
    let origin = origin
        .to_str()
        .map_err(|_| AppError::Forbidden("invalid Origin header".into()))?
        .trim_end_matches('/');

    if state
        .cfg
        .allowed_origins
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    {
        return Ok(());
    }
    if let Some(public) = &state.cfg.public_url {
        if public.eq_ignore_ascii_case(origin) {
            return Ok(());
        }
    }

    let Some(request_host) = request_host(state, req.headers()) else {
        return Err(AppError::Forbidden("missing Host header".into()));
    };
    let origin_host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);

    if origin_host.eq_ignore_ascii_case(&request_host) {
        Ok(())
    } else {
        tracing::warn!(%origin, %request_host, "rejected cross-origin request");
        Err(AppError::Forbidden(
            "cross-origin requests are not allowed".into(),
        ))
    }
}

fn request_host(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if state.cfg.trust_proxy {
        if let Some(forwarded) = headers
            .get("x-forwarded-host")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
        {
            let host = forwarded.trim();
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
    }
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
}

fn client_ip(
    state: &AppState,
    headers: &HeaderMap,
    connect_info: Option<&ConnectInfo<SocketAddr>>,
) -> Option<IpAddr> {
    if state.cfg.trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
        {
            return Some(ip);
        }
    }
    connect_info.map(|info| info.0.ip())
}
