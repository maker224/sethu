//! End-to-end Android bridge test using a fake `adb` executable. No device or
//! Android platform-tools required.

use std::{net::SocketAddr, os::unix::fs::PermissionsExt, time::Duration};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sethu_server::{app, config::Config, state::AppState};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use uuid::Uuid;

/// A valid 1x1 PNG.
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test]
async fn android_bridge_streams_frames_and_injects_input() {
    let dir = std::env::temp_dir().join(format!("sethu-android-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let png_path = dir.join("shot.png");
    let log_path = dir.join("adb.log");
    let png = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG_B64)
        .expect("png");
    std::fs::write(&png_path, &png).expect("write png");

    let script = dir.join("adb");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
echo "$@" >> "{}"
case "$*" in
  *"exec-out screencap -p"*) cat "{}" ;;
  *"get-state"*) echo device ;;
  *"shell input "*) exit 0 ;;
  *"push "*) exit 0 ;;
  *) echo "unexpected adb args: $*" >&2; exit 1 ;;
esac
"#,
            log_path.display(),
            png_path.display()
        ),
    )
    .expect("write script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut config = Config::for_tests(dir.clone());
    config.adb_path = script.to_string_lossy().to_string();
    config.adb_timeout = Duration::from_secs(3);
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

    let base = format!("http://{addr}");
    let client = reqwest::Client::new();
    let register = client
        .post(format!("{base}/api/auth/register"))
        .header("origin", &base)
        .json(&json!({"email": "android@example.com", "password": "android-password"}))
        .send()
        .await
        .expect("register");
    assert_eq!(register.status(), reqwest::StatusCode::CREATED);
    let cookie = register
        .headers()
        .get("set-cookie")
        .expect("cookie")
        .to_str()
        .expect("cookie text")
        .split(';')
        .next()
        .expect("pair")
        .to_string();

    let session: Value = client
        .get(format!("{base}/api/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session")
        .json()
        .await
        .expect("session json");
    let csrf = session["csrf"].as_str().expect("csrf").to_string();

    let created: Value = client
        .post(format!("{base}/api/hosts"))
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .header("origin", &base)
        .json(&json!({
            "name": "Fake Android",
            "kind": "android",
            "target": "fake-serial",
        }))
        .send()
        .await
        .expect("create host")
        .json()
        .await
        .expect("host json");
    let host_id = created["id"].as_str().expect("host id").to_string();
    assert_eq!(created["target"], "fake-serial");

    // Reachability check goes through adb get-state.
    let tested: Value = client
        .post(format!("{base}/api/hosts/{host_id}/test"))
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .header("origin", &base)
        .send()
        .await
        .expect("test host")
        .json()
        .await
        .expect("test json");
    assert_eq!(tested["ok"], true);

    let mut request = format!("ws://{addr}/ws/android/{host_id}")
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("cookie", cookie.parse().expect("cookie header"));
    request
        .headers_mut()
        .insert("origin", base.parse().expect("origin header"));
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws connect");

    let mut widths = Vec::new();
    let mut got_png = false;
    for _ in 0..4 {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("timeout")
            .expect("message")
            .expect("ok message");
        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_str()).expect("json");
                if value["type"] == "ready" {
                    widths.push(value["width"].as_u64().expect("width"));
                    assert_eq!(value["scale"], 1);
                }
            }
            Message::Binary(bytes) => {
                assert_eq!(&bytes[..], &png[..]);
                got_png = true;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(widths, vec![1]);
    assert!(got_png, "expected a binary PNG frame");

    socket
        .send(Message::Text(
            json!({"type": "tap", "x": 0, "y": 0}).to_string().into(),
        ))
        .await
        .expect("tap");
    socket
        .send(Message::Text(
            json!({"type": "keys", "text": "hi"}).to_string().into(),
        ))
        .await
        .expect("keys");
    socket
        .send(Message::Text(
            json!({"type": "button", "name": "home"}).to_string().into(),
        ))
        .await
        .expect("button");

    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = std::fs::read_to_string(&log_path).expect("adb log");
    assert!(
        log.contains("shell input tap 0 0"),
        "missing tap in log:\n{log}"
    );
    assert!(
        log.contains("shell input text hi"),
        "missing text in log:\n{log}"
    );
    assert!(
        log.contains("shell input keyevent 3"),
        "missing keyevent in log:\n{log}"
    );

    // MacBook → Android: upload a file, then push it to Downloads via adb.
    let part = reqwest::multipart::Part::bytes(b"note from mac".to_vec())
        .file_name("note.txt")
        .mime_str("text/plain")
        .expect("mime");
    let uploaded: Value = client
        .post(format!("{base}/api/files"))
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .header("origin", &base)
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("upload json");
    let file_id = uploaded["id"].as_str().expect("file id").to_string();

    let sent = client
        .post(format!("{base}/api/files/{file_id}/send"))
        .header("cookie", &cookie)
        .header("x-sethu-csrf", &csrf)
        .header("origin", &base)
        .json(&json!({"host_id": host_id}))
        .send()
        .await
        .expect("send file");
    assert_eq!(sent.status(), reqwest::StatusCode::OK);
    let sent_body: Value = sent.json().await.expect("send json");
    assert_eq!(sent_body["ok"], true);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = std::fs::read_to_string(&log_path).expect("adb log");
    assert!(
        log.contains("push") && log.contains("/sdcard/Download/note.txt"),
        "missing adb push in log:\n{log}"
    );

    let _ = socket.close(None).await;
    server.abort();
    let _ = std::fs::remove_dir_all(dir);
}
