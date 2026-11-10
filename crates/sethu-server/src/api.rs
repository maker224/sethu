//! JSON API consumed by the web frontend.

use std::time::Instant;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpStream;
use uuid::Uuid;

use crate::{
    auth::{self, AuthUser},
    error::AppError,
    state::AppState,
    store::{Host, HostKind},
    target,
    wda::WdaClient,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/session", get(session_info))
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/hosts", get(list_hosts).post(create_host))
        .route(
            "/api/hosts/{id}",
            get(get_host).patch(update_host).delete(delete_host),
        )
        .route("/api/hosts/{id}/test", post(test_host))
}

// ----- views -------------------------------------------------------------

#[derive(Serialize)]
struct UserView {
    id: Uuid,
    email: String,
    created_at: i64,
}

impl From<&crate::store::User> for UserView {
    fn from(user: &crate::store::User) -> Self {
        Self {
            id: user.id,
            email: user.email.clone(),
            created_at: user.created_at,
        }
    }
}

#[derive(Serialize)]
struct SessionView {
    user: Option<UserView>,
    csrf: String,
    registration: bool,
    version: &'static str,
    cookie_secure: bool,
    /// Whether a locally vendored noVNC copy is available.
    novnc_vendored: bool,
}

#[derive(Serialize)]
struct HostView {
    id: Uuid,
    name: String,
    kind: HostKind,
    target: String,
    notes: Option<String>,
    created_at: i64,
}

impl From<Host> for HostView {
    fn from(host: Host) -> Self {
        Self {
            id: host.id,
            name: host.name,
            kind: host.kind,
            target: host.target,
            notes: host.notes,
            created_at: host.created_at,
        }
    }
}

// ----- health & session --------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn session_info(State(state): State<AppState>, headers: HeaderMap) -> Json<SessionView> {
    let found = auth::session_from_headers(&state, &headers);
    let (user, csrf) = match found {
        Some((session, user)) => (Some(UserView::from(&user)), session.csrf),
        None => (None, String::new()),
    };
    Json(SessionView {
        user,
        csrf,
        registration: state.cfg.allow_registration || state.store.user_count() == 0,
        version: env!("CARGO_PKG_VERSION"),
        cookie_secure: state.cfg.cookie_secure,
        novnc_vendored: state.cfg.vendor_dir.join("novnc/core/rfb.js").is_file(),
    })
}

// ----- auth --------------------------------------------------------------

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Response, AppError> {
    // With zero accounts the server must remain claimable even if registration
    // is disabled, otherwise a fresh deployment would be impossible to set up.
    let first_user = state.store.user_count() == 0;
    if !state.cfg.allow_registration && !first_user {
        return Err(AppError::Forbidden(
            "account registration is disabled on this server".into(),
        ));
    }
    let email = auth::normalize_email(&body.email)?;
    auth::validate_password(&body.password)?;
    if state.store.user_by_email(&email).is_some() {
        return Err(AppError::Conflict(
            "an account with that email exists".into(),
        ));
    }

    let hash = auth::hash_password(body.password).await?;
    let user = state.store.create_user(&email, hash)?;
    let (token, _session) = state.store.create_session(user.id, state.cfg.session_ttl)?;
    tracing::info!(user_id = %user.id, "account created");

    Ok(json_with_cookie(
        StatusCode::CREATED,
        auth::session_cookie(&state, &token),
        &UserView::from(&user),
    ))
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_ascii_lowercase();
    let user = state.store.user_by_email(&email);

    // Always spend the same argon2 work to avoid leaking which emails exist.
    let hash = match &user {
        Some(user) => user.password_hash.clone(),
        None => auth::dummy_password_hash().to_string(),
    };
    let password_ok = auth::verify_password(hash, body.password).await;

    let Some(user) = user.filter(|_| password_ok) else {
        tracing::warn!(email = %email, "failed login");
        return Err(AppError::Unauthorized);
    };

    let (token, _session) = state.store.create_session(user.id, state.cfg.session_ttl)?;
    tracing::info!(user_id = %user.id, "login");
    Ok(json_with_cookie(
        StatusCode::OK,
        auth::session_cookie(&state, &token),
        &UserView::from(&user),
    ))
}

async fn logout(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    auth::delete_current_session(&state, &headers)?;
    tracing::info!(user_id = %user.user.id, "logout");
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(&auth::clear_cookie(&state)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

// ----- hosts -------------------------------------------------------------

async fn list_hosts(State(state): State<AppState>, user: AuthUser) -> Json<Vec<HostView>> {
    let hosts = state
        .store
        .list_hosts(user.user.id)
        .into_iter()
        .map(HostView::from)
        .collect();
    Json(hosts)
}

async fn get_host(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<HostView>, AppError> {
    let host = state
        .store
        .host(id, user.user.id)
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    Ok(Json(HostView::from(host)))
}

#[derive(Deserialize)]
struct CreateHost {
    name: String,
    kind: HostKind,
    target: String,
    #[serde(default)]
    notes: Option<String>,
}

async fn create_host(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateHost>,
) -> Result<Response, AppError> {
    let name = clean_name(&body.name)?;
    let target = match body.kind {
        HostKind::Macos => target::normalize_vnc(&body.target)?,
        HostKind::Ios => target::normalize_wda(&body.target)?,
    };
    let notes = clean_notes(body.notes);

    let host = state
        .store
        .create_host(user.user.id, name, body.kind, target, notes)?;
    tracing::info!(user_id = %user.user.id, host_id = %host.id, "host added");
    Ok((StatusCode::CREATED, Json(HostView::from(host))).into_response())
}

#[derive(Deserialize)]
struct UpdateHost {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

async fn update_host(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateHost>,
) -> Result<Json<HostView>, AppError> {
    let name = match body.name {
        Some(name) => Some(clean_name(&name)?),
        None => None,
    };
    let notes = body.notes.map(|notes| clean_notes(Some(notes)));
    let host = state.store.update_host(id, user.user.id, name, notes)?;
    Ok(Json(HostView::from(host)))
}

async fn delete_host(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    state.store.delete_host(id, user.user.id)?;
    tracing::info!(user_id = %user.user.id, host_id = %id, "host removed");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TestResult {
    ok: bool,
    latency_ms: u128,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready: Option<bool>,
}

async fn test_host(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<TestResult>, AppError> {
    let host = state
        .store
        .host(id, user.user.id)
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;

    match host.kind {
        HostKind::Macos => {
            let started = Instant::now();
            let attempt = tokio::time::timeout(
                state.cfg.vnc_connect_timeout,
                TcpStream::connect(&host.target),
            )
            .await;
            let latency_ms = started.elapsed().as_millis();
            let result = match attempt {
                Ok(Ok(_stream)) => TestResult {
                    ok: true,
                    latency_ms,
                    message: "TCP connection succeeded. The viewer will ask for your macOS account or the VNC password configured in Screen Sharing.".into(),
                    ready: None,
                },
                Ok(Err(err)) => TestResult {
                    ok: false,
                    latency_ms,
                    message: format!("Could not connect to {}: {err}", host.target),
                    ready: None,
                },
                Err(_) => TestResult {
                    ok: false,
                    latency_ms,
                    message: format!("Timed out connecting to {}", host.target),
                    ready: None,
                },
            };
            Ok(Json(result))
        }
        HostKind::Ios => {
            let started = Instant::now();
            let client =
                WdaClient::from_target(&host.target, state.http.clone(), state.cfg.wda_timeout)?;
            let result = match client.status().await {
                Ok(status) => TestResult {
                    ok: true,
                    latency_ms: started.elapsed().as_millis(),
                    message: status
                        .pointer("/value/message")
                        .and_then(|value| value.as_str())
                        .map(|message| format!("WebDriverAgent responded: {message}"))
                        .unwrap_or_else(|| "WebDriverAgent is reachable".into()),
                    ready: status
                        .pointer("/value/ready")
                        .and_then(|value| value.as_bool()),
                },
                Err(err) => TestResult {
                    ok: false,
                    latency_ms: started.elapsed().as_millis(),
                    message: err.to_string(),
                    ready: None,
                },
            };
            Ok(Json(result))
        }
    }
}

// ----- helpers -----------------------------------------------------------

fn clean_name(raw: &str) -> Result<String, AppError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("give this device a name".into()));
    }
    if name.chars().count() > 80 {
        return Err(AppError::BadRequest(
            "device name must be 80 characters or fewer".into(),
        ));
    }
    Ok(name.to_string())
}

fn clean_notes(notes: Option<String>) -> Option<String> {
    let notes = notes?.trim().to_string();
    if notes.is_empty() {
        None
    } else {
        Some(notes.chars().take(280).collect())
    }
}

fn json_with_cookie<T: Serialize>(status: StatusCode, cookie: String, body: &T) -> Response {
    let mut response = (status, Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}
