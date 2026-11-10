//! End-to-end API tests: registration, sessions, CSRF, origin checks and host
//! management. These run without a network or a real device.

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sethu_server::{app, config::Config, state::AppState};
use tower::ServiceExt;
use uuid::Uuid;

struct Harness {
    app: Router,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn harness() -> Harness {
    let dir = std::env::temp_dir().join(format!("sethu-api-{}", Uuid::new_v4()));
    let state = AppState::new(Config::for_tests(dir.clone())).expect("state");
    Harness {
        app: app(state),
        dir,
    }
}

struct Reply {
    status: StatusCode,
    token: Option<String>,
    body: Value,
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
) -> Reply {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "sethu.test")
        .header(header::ORIGIN, origin.unwrap_or("http://sethu.test"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if path.starts_with("/ws/") {
        builder = builder
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    }
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, format!("sethu_session={token}"));
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-sethu-csrf", csrf);
    }
    let request = builder
        .body(Body::from(
            body.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .expect("request");

    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let token = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| cookie.split(';').next())
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty());
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        token,
        body,
    }
}

#[tokio::test]
async fn health_and_anonymous_session() {
    let app = harness();
    let health = call(&app.app, "GET", "/api/health", None, None, None, None).await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.body["status"], "ok");

    let session = call(&app.app, "GET", "/api/session", None, None, None, None).await;
    assert_eq!(session.status, StatusCode::OK);
    assert!(session.body["user"].is_null());
    assert_eq!(session.body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn registration_rejects_weak_passwords() {
    let app = harness();
    let reply = call(
        &app.app,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "owner@example.com", "password": "short"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    let reply = call(
        &app.app,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "not-an-email", "password": "correct horse battery"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn full_auth_and_host_lifecycle() {
    let app = harness();

    let register = call(
        &app.app,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "Owner@Example.com", "password": "correct horse battery"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(register.status, StatusCode::CREATED);
    assert_eq!(register.body["email"], "owner@example.com");
    let token = register.token.expect("session cookie");

    // Duplicate accounts are rejected.
    let duplicate = call(
        &app.app,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "owner@example.com", "password": "correct horse battery"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);

    let session = call(
        &app.app,
        "GET",
        "/api/session",
        None,
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(session.body["user"]["email"], "owner@example.com");
    let csrf = session.body["csrf"].as_str().expect("csrf").to_string();
    assert!(!csrf.is_empty());

    // Mutations need the CSRF token.
    let no_csrf = call(
        &app.app,
        "POST",
        "/api/hosts",
        Some(json!({"name": "Mac", "kind": "macos", "target": "mac.local"})),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);

    let created = call(
        &app.app,
        "POST",
        "/api/hosts",
        Some(json!({
            "name": "Studio Mac",
            "kind": "macos",
            "target": "mac.local",
            "notes": "living room"
        })),
        Some(&token),
        Some(&csrf),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["target"], "mac.local:5900");
    assert_eq!(created.body["kind"], "macos");
    let host_id = created.body["id"].as_str().expect("host id").to_string();

    let phone = call(
        &app.app,
        "POST",
        "/api/hosts",
        Some(json!({
            "name": "iPhone",
            "kind": "ios",
            "target": " http://192.168.1.9:8100/ "
        })),
        Some(&token),
        Some(&csrf),
        None,
    )
    .await;
    assert_eq!(phone.status, StatusCode::CREATED);
    assert_eq!(phone.body["target"], "http://192.168.1.9:8100");

    let listed = call(
        &app.app,
        "GET",
        "/api/hosts",
        None,
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(listed.body.as_array().expect("array").len(), 2);

    // Requests from another origin are rejected before reaching the handler.
    let cross_origin = call(
        &app.app,
        "GET",
        "/api/hosts",
        None,
        Some(&token),
        None,
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(cross_origin.status, StatusCode::FORBIDDEN);

    // Invalid device kinds cannot be smuggled into the wrong endpoint.
    let bad_target = call(
        &app.app,
        "POST",
        "/api/hosts",
        Some(json!({"name": "Wrong", "kind": "macos", "target": "http://x"})),
        Some(&token),
        Some(&csrf),
        None,
    )
    .await;
    assert_eq!(bad_target.status, StatusCode::BAD_REQUEST);

    // Removing a host requires auth and CSRF.
    let removed = call(
        &app.app,
        "DELETE",
        &format!("/api/hosts/{host_id}"),
        None,
        Some(&token),
        Some(&csrf),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);

    // Wrong password does not create a session.
    let bad_login = call(
        &app.app,
        "POST",
        "/api/auth/login",
        Some(json!({"email": "owner@example.com", "password": "wrong password"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(bad_login.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        bad_login.body["error"]["message"],
        "invalid email or password"
    );
    assert!(bad_login.token.is_none());

    // Correct password rotates to a fresh session.
    let login = call(
        &app.app,
        "POST",
        "/api/auth/login",
        Some(json!({"email": "owner@example.com", "password": "correct horse battery"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
    let login_token = login.token.expect("cookie");

    let login_session = call(
        &app.app,
        "GET",
        "/api/session",
        None,
        Some(&login_token),
        None,
        None,
    )
    .await;
    let login_csrf = login_session.body["csrf"]
        .as_str()
        .expect("csrf")
        .to_string();

    let logout = call(
        &app.app,
        "POST",
        "/api/auth/logout",
        None,
        Some(&login_token),
        Some(&login_csrf),
        None,
    )
    .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);

    let after_logout = call(
        &app.app,
        "GET",
        "/api/session",
        None,
        Some(&login_token),
        None,
        None,
    )
    .await;
    assert!(after_logout.body["user"].is_null());
}

#[tokio::test]
async fn first_account_bootstraps_when_registration_is_closed() {
    let dir = std::env::temp_dir().join(format!("sethu-bootstrap-{}", Uuid::new_v4()));
    let mut config = Config::for_tests(dir.clone());
    config.allow_registration = false;
    let router = app(AppState::new(config).expect("state"));

    let first = call(
        &router,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "first@example.com", "password": "bootstrap-password"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    let second = call(
        &router,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "second@example.com", "password": "bootstrap-password"})),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(second.status, StatusCode::FORBIDDEN);

    let session = call(&router, "GET", "/api/session", None, None, None, None).await;
    assert_eq!(session.body["registration"], false);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn protected_surfaces_require_auth_and_same_origin() {
    let app = harness();
    let unknown = call(
        &app.app,
        "GET",
        "/api/hosts/00000000-0000-4000-8000-000000000000",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);

    // Cross-origin WebSocket attempts are rejected by the guard middleware.
    let cross_origin_ws = call(
        &app.app,
        "GET",
        "/ws/vnc/00000000-0000-4000-8000-000000000000",
        None,
        None,
        None,
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(cross_origin_ws.status, StatusCode::FORBIDDEN);

    // A same-origin request reaches the WebSocket extractor (which, without a
    // real hyper upgrade, answers 426 rather than leaking any device data).
    let ws = call(
        &app.app,
        "GET",
        "/ws/vnc/00000000-0000-4000-8000-000000000000",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(ws.status, StatusCode::UPGRADE_REQUIRED);
}
