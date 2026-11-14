//! Cross-network presence and device-to-device transfer tests.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

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
    Config::for_tests(dir.to_path_buf())
}

async fn register(harness: &Harness, email: &str) -> (String, String) {
    let response = harness
        .client
        .post(format!("{}/api/auth/register", harness.base))
        .header("origin", &harness.base)
        .json(&json!({"email": email, "password": "transfer-password"}))
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

async fn post_json(
    harness: &Harness,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: Value,
) -> reqwest::Response {
    let mut request = harness
        .client
        .post(format!("{}{path}", harness.base))
        .header("origin", &harness.base)
        .json(&body);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-sethu-csrf", csrf);
    }
    request.send().await.expect("request")
}

async fn upload(harness: &Harness, cookie: &str, csrf: &str, name: &str, bytes: &[u8]) -> String {
    let part = Part::bytes(bytes.to_vec())
        .file_name(name.to_string())
        .mime_str("application/octet-stream")
        .expect("mime");
    let response = harness
        .client
        .post(format!("{}/api/files", harness.base))
        .header("origin", &harness.base)
        .header("cookie", cookie)
        .header("x-sethu-csrf", csrf)
        .multipart(Form::new().part("file", part))
        .send()
        .await
        .expect("upload");
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json::<Value>().await.expect("file")["id"]
        .as_str()
        .expect("file id")
        .to_string()
}

async fn go_online(harness: &Harness, cookie: &str, csrf: &str, id: Uuid, name: &str) {
    let response = post_json(
        harness,
        "/api/presence",
        Some(cookie),
        Some(csrf),
        json!({"device_id": id, "name": name}),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn devices_must_both_be_online_to_exchange_a_file() {
    let dir = std::env::temp_dir().join(format!("sethu-transfer-{}", Uuid::new_v4()));
    let harness = start(config(&dir)).await;
    let (cookie, csrf) = register(&harness, "transfer@example.com").await;
    let file_id = upload(&harness, &cookie, &csrf, "holiday.mov", b"video-bytes").await;
    let mac = Uuid::new_v4();
    let phone = Uuid::new_v4();

    // Nobody has announced presence yet: the send is refused.
    let early = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&cookie),
        Some(&csrf),
        json!({"from_device": mac, "to_device": phone}),
    )
    .await;
    assert_eq!(early.status(), reqwest::StatusCode::CONFLICT);

    // One device online is not enough.
    go_online(&harness, &cookie, &csrf, mac, "Studio MacBook").await;
    let half = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&cookie),
        Some(&csrf),
        json!({"from_device": mac, "to_device": phone}),
    )
    .await;
    assert_eq!(half.status(), reqwest::StatusCode::CONFLICT);
    let message = half.json::<Value>().await.expect("error json")["error"]["message"]
        .as_str()
        .expect("message")
        .to_string();
    assert!(message.contains("offline"));

    // Both online: the transfer is queued, not delivered.
    go_online(&harness, &cookie, &csrf, phone, "Pixel 9").await;
    let queued = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&cookie),
        Some(&csrf),
        json!({"from_device": mac, "to_device": phone}),
    )
    .await;
    assert_eq!(queued.status(), reqwest::StatusCode::CREATED);
    let queued: Value = queued.json().await.expect("transfer json");
    assert_eq!(queued["status"], "pending");
    assert_eq!(queued["from"]["name"], "Studio MacBook");
    assert_eq!(queued["to"]["name"], "Pixel 9");
    assert!(queued["download_url"].is_null());
    let transfer_id = queued["id"].as_str().expect("transfer id").to_string();

    // Both devices see it in the workspace list.
    let listed: Value = harness
        .client
        .get(format!("{}/api/transfers", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("list transfers")
        .json()
        .await
        .expect("transfer list json");
    assert_eq!(listed.as_array().expect("array").len(), 1);

    // It cannot be downloaded before the receiving device accepts.
    let premature = harness
        .client
        .get(format!(
            "{}/api/transfers/{transfer_id}/download",
            harness.base
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("premature download");
    assert_eq!(premature.status(), reqwest::StatusCode::FORBIDDEN);

    // Only the recipient device may accept.
    let wrong_device = post_json(
        &harness,
        &format!("/api/transfers/{transfer_id}/accept"),
        Some(&cookie),
        Some(&csrf),
        json!({"device_id": mac}),
    )
    .await;
    assert_eq!(wrong_device.status(), reqwest::StatusCode::FORBIDDEN);

    let accepted = post_json(
        &harness,
        &format!("/api/transfers/{transfer_id}/accept"),
        Some(&cookie),
        Some(&csrf),
        json!({"device_id": phone}),
    )
    .await;
    assert_eq!(accepted.status(), reqwest::StatusCode::OK);
    let accepted: Value = accepted.json().await.expect("accepted json");
    assert_eq!(accepted["status"], "delivered");
    assert_eq!(
        accepted["download_url"],
        format!("/api/transfers/{transfer_id}/download")
    );

    let download = harness
        .client
        .get(format!(
            "{}/api/transfers/{transfer_id}/download",
            harness.base
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("download");
    assert_eq!(download.status(), reqwest::StatusCode::OK);
    assert_eq!(&download.bytes().await.expect("bytes")[..], b"video-bytes");

    // Accepting twice is refused; the transfer is finished.
    let replay = post_json(
        &harness,
        &format!("/api/transfers/{transfer_id}/accept"),
        Some(&cookie),
        Some(&csrf),
        json!({"device_id": phone}),
    )
    .await;
    assert_eq!(replay.status(), reqwest::StatusCode::CONFLICT);

    // Once the phone goes offline, no new transfer can target it.
    go_online(&harness, &cookie, &csrf, mac, "Studio MacBook").await;
    let bye = harness
        .client
        .delete(format!("{}/api/presence/{phone}", harness.base))
        .header("origin", &harness.base)
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .send()
        .await
        .expect("offline");
    assert_eq!(bye.status(), reqwest::StatusCode::NO_CONTENT);
    let offline = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&cookie),
        Some(&csrf),
        json!({"from_device": mac, "to_device": phone}),
    )
    .await;
    assert_eq!(offline.status(), reqwest::StatusCode::CONFLICT);
}

#[tokio::test]
async fn presence_and_transfers_stay_inside_the_workspace() {
    let dir = std::env::temp_dir().join(format!("sethu-transfer-iso-{}", Uuid::new_v4()));
    let harness = start(config(&dir)).await;
    let (alice_cookie, alice_csrf) = register(&harness, "alice-transfer@example.com").await;
    let (bob_cookie, bob_csrf) = register(&harness, "bob-transfer@example.com").await;
    let file_id = upload(
        &harness,
        &alice_cookie,
        &alice_csrf,
        "alice.pdf",
        b"alice only",
    )
    .await;
    let alice_phone = Uuid::new_v4();
    let bob_mac = Uuid::new_v4();
    let bob_phone = Uuid::new_v4();
    go_online(
        &harness,
        &alice_cookie,
        &alice_csrf,
        alice_phone,
        "Alice iPhone",
    )
    .await;
    go_online(&harness, &bob_cookie, &bob_csrf, bob_mac, "Bob Mac").await;
    go_online(&harness, &bob_cookie, &bob_csrf, bob_phone, "Bob Phone").await;

    // Bob cannot see Alice's online devices.
    let bob_presence: Value = harness
        .client
        .get(format!("{}/api/presence", harness.base))
        .header("cookie", &bob_cookie)
        .send()
        .await
        .expect("presence")
        .json()
        .await
        .expect("presence json");
    let names = bob_presence["peers"]
        .as_array()
        .expect("peers")
        .iter()
        .map(|peer| peer["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["Bob Mac", "Bob Phone"]);

    // Alice cannot send her file to Bob's device, even while both are online.
    let transfer = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&alice_cookie),
        Some(&alice_csrf),
        json!({"from_device": alice_phone, "to_device": bob_mac}),
    )
    .await;
    assert_eq!(transfer.status(), reqwest::StatusCode::CONFLICT);

    // Bob cannot use Alice's file id either.
    let bob_file = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&bob_cookie),
        Some(&bob_csrf),
        json!({"from_device": bob_mac, "to_device": bob_phone}),
    )
    .await;
    assert_eq!(bob_file.status(), reqwest::StatusCode::NOT_FOUND);

    // Sending to yourself is refused outright.
    let self_transfer = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&alice_cookie),
        Some(&alice_csrf),
        json!({"from_device": alice_phone, "to_device": alice_phone}),
    )
    .await;
    assert_eq!(self_transfer.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn presence_requires_a_session_and_csrf() {
    let dir = std::env::temp_dir().join(format!("sethu-transfer-auth-{}", Uuid::new_v4()));
    let harness = start(config(&dir)).await;
    let (cookie, _csrf) = register(&harness, "auth-transfer@example.com").await;
    let device = Uuid::new_v4();

    let anonymous = post_json(
        &harness,
        "/api/presence",
        None,
        None,
        json!({"device_id": device, "name": "Unknown"}),
    )
    .await;
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);

    let no_csrf = post_json(
        &harness,
        "/api/presence",
        Some(&cookie),
        None,
        json!({"device_id": device, "name": "Unknown"}),
    )
    .await;
    assert_eq!(no_csrf.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_expired_transfer_cannot_be_accepted() {
    let dir = std::env::temp_dir().join(format!("sethu-transfer-expire-{}", Uuid::new_v4()));
    let mut config = config(&dir);
    config.transfer_ttl = Duration::from_secs(1);
    let harness = start(config).await;
    let (cookie, csrf) = register(&harness, "expire@example.com").await;
    let file_id = upload(&harness, &cookie, &csrf, "soon.bin", b"1234").await;
    let from = Uuid::new_v4();
    let to = Uuid::new_v4();
    go_online(&harness, &cookie, &csrf, from, "Mac").await;
    go_online(&harness, &cookie, &csrf, to, "Phone").await;

    let queued: Value = post_json(
        &harness,
        &format!("/api/files/{file_id}/transfer"),
        Some(&cookie),
        Some(&csrf),
        json!({"from_device": from, "to_device": to}),
    )
    .await
    .json()
    .await
    .expect("transfer json");
    let id = queued["id"].as_str().expect("id").to_string();

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    let expired = post_json(
        &harness,
        &format!("/api/transfers/{id}/accept"),
        Some(&cookie),
        Some(&csrf),
        json!({"device_id": to}),
    )
    .await;
    assert_eq!(expired.status(), reqwest::StatusCode::CONFLICT);

    let listed: Value = harness
        .client
        .get(format!("{}/api/transfers", harness.base))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    assert_eq!(listed[0]["status"], "expired");
}
