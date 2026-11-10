//! File workspace integration tests: multipart upload, download, deletion,
//! size limits and open-access mode.

use std::{net::SocketAddr, path::PathBuf};

use reqwest::{
    multipart::{Form, Part},
    Client,
};
use serde_json::{json, Value};
use sethu_server::{app, config::Config, state::AppState};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

struct Harness {
    base: String,
    client: Client,
    server: JoinHandle<()>,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(config: Config) -> Harness {
    let dir = config.data_dir.clone();
    let state = AppState::new(config).expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    Harness {
        base: format!("http://{addr}"),
        client: Client::new(),
        server,
        dir,
    }
}

fn config(dir: &std::path::Path) -> Config {
    let mut config = Config::for_tests(dir.to_path_buf());
    config.max_file_bytes = 1024 * 1024;
    config
}

async fn register(harness: &Harness) -> (String, String) {
    let response = harness
        .client
        .post(format!("{}/api/auth/register", harness.base))
        .header("origin", &harness.base)
        .json(&json!({"email": "files@example.com", "password": "files-password"}))
        .send()
        .await
        .expect("register");
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let cookie = response
        .headers()
        .get("set-cookie")
        .expect("cookie")
        .to_str()
        .expect("cookie text")
        .split(';')
        .next()
        .expect("pair")
        .to_string();
    let session: Value = harness
        .client
        .get(format!("{}/api/session", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session")
        .json()
        .await
        .expect("session json");
    let csrf = session["csrf"].as_str().expect("csrf").to_string();
    (cookie, csrf)
}

async fn upload(
    harness: &Harness,
    cookie: Option<&str>,
    csrf: Option<&str>,
    name: &str,
    bytes: Vec<u8>,
) -> reqwest::Response {
    let mut request = harness
        .client
        .post(format!("{}/api/files", harness.base))
        .header("origin", &harness.base);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-sethu-csrf", csrf);
    }
    let part = Part::bytes(bytes)
        .file_name(name.to_string())
        .mime_str("application/pdf")
        .expect("mime");
    request
        .multipart(Form::new().part("file", part))
        .send()
        .await
        .expect("upload")
}

#[tokio::test]
async fn upload_list_download_delete_roundtrip() {
    let dir = std::env::temp_dir().join(format!("sethu-files-{}", Uuid::new_v4()));
    let harness = start(config(&dir)).await;
    let (cookie, csrf) = register(&harness).await;

    // Deliberately larger than the 64 KiB JSON body limit.
    let payload = vec![0xABu8; 100_000];
    let response = upload(
        &harness,
        Some(&cookie),
        Some(&csrf),
        "../../secret report.pdf",
        payload.clone(),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let created: Value = response.json().await.expect("created json");
    assert_eq!(created["name"], "secret report.pdf");
    assert_eq!(created["size"], payload.len());
    assert_eq!(created["mime"], "application/pdf");
    let id = created["id"].as_str().expect("id").to_string();

    let list: Value = harness
        .client
        .get(format!("{}/api/files", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    assert_eq!(list.as_array().expect("array").len(), 1);

    // CSRF is enforced for uploads in secure mode.
    let no_csrf = upload(&harness, Some(&cookie), None, "x.txt", vec![1]).await;
    assert_eq!(no_csrf.status(), reqwest::StatusCode::FORBIDDEN);

    // Files require a session.
    let anonymous = upload(&harness, None, None, "x.txt", vec![1]).await;
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);

    let download = harness
        .client
        .get(format!("{}/api/files/{id}/download", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("download");
    assert_eq!(download.status(), reqwest::StatusCode::OK);
    let disposition = download
        .headers()
        .get("content-disposition")
        .expect("disposition")
        .to_str()
        .expect("disposition text")
        .to_string();
    assert!(disposition.starts_with("attachment;"));
    assert!(disposition.contains("filename*=UTF-8''"));
    assert_eq!(download.bytes().await.expect("bytes"), payload);

    let deleted = harness
        .client
        .delete(format!("{}/api/files/{id}", harness.base))
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .header("origin", &harness.base)
        .send()
        .await
        .expect("delete");
    assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);

    let missing = harness
        .client
        .get(format!("{}/api/files/{id}/download", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("missing");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oversize_upload_is_rejected_and_cleaned_up() {
    let dir = std::env::temp_dir().join(format!("sethu-files-big-{}", Uuid::new_v4()));
    let mut config = Config::for_tests(dir.clone());
    config.max_file_bytes = 1_024;
    let harness = start(config).await;
    let (cookie, csrf) = register(&harness).await;

    let response = upload(
        &harness,
        Some(&cookie),
        Some(&csrf),
        "large.bin",
        vec![7u8; 4_096],
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    let list: Value = harness
        .client
        .get(format!("{}/api/files", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    assert_eq!(list.as_array().expect("array").len(), 0);

    // The partial file must not be left behind.
    let files_root = harness.dir.join("files");
    let mut leftover = 0usize;
    if let Ok(users) = std::fs::read_dir(&files_root) {
        for user in users.flatten() {
            if let Ok(entries) = std::fs::read_dir(user.path()) {
                leftover += entries.flatten().count();
            }
        }
    }
    assert_eq!(leftover, 0, "partial uploads should be removed");
}

#[tokio::test]
async fn open_mode_needs_no_sign_in_or_csrf() {
    let dir = std::env::temp_dir().join(format!("sethu-open-{}", Uuid::new_v4()));
    let mut config = config(&dir);
    config.open_access = true;
    let harness = start(config).await;

    let session: Value = harness
        .client
        .get(format!("{}/api/session", harness.base))
        .send()
        .await
        .expect("session")
        .json()
        .await
        .expect("session json");
    assert_eq!(session["auth_open"], true);
    assert!(session["user"]["email"].is_string());

    let host = harness
        .client
        .post(format!("{}/api/hosts", harness.base))
        .header("origin", &harness.base)
        .json(&json!({"name": "Mac", "kind": "macos", "target": "mac.local"}))
        .send()
        .await
        .expect("create host");
    assert_eq!(host.status(), reqwest::StatusCode::CREATED);

    let uploaded = upload(&harness, None, None, "from-phone.txt", b"hello".to_vec()).await;
    assert_eq!(uploaded.status(), reqwest::StatusCode::CREATED);

    let login = harness
        .client
        .post(format!("{}/api/auth/login", harness.base))
        .header("origin", &harness.base)
        .json(&json!({"email": "any@example.com", "password": "irrelevant-password"}))
        .send()
        .await
        .expect("login");
    assert_eq!(login.status(), reqwest::StatusCode::FORBIDDEN);

    let download = harness
        .client
        .get(format!("{}/api/files", harness.base))
        .send()
        .await
        .expect("list")
        .json::<Value>()
        .await
        .expect("list json");
    let id = download[0]["id"].as_str().expect("id");
    let bytes = harness
        .client
        .get(format!("{}/api/files/{id}/download", harness.base))
        .send()
        .await
        .expect("download")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(&bytes[..], b"hello");
}
