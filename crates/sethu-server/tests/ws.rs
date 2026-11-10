//! End-to-end WebSocket test: the VNC bridge must relay bytes between a
//! browser and a TCP device without interpreting RFB.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sethu_server::{app, config::Config, state::AppState};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use uuid::Uuid;

#[tokio::test]
async fn vnc_bridge_relays_bytes_end_to_end() {
    // A fake RFB server: echoes whatever arrives.
    let device = TcpListener::bind("127.0.0.1:0").await.expect("device bind");
    let device_addr = device.local_addr().expect("device addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = device.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = socket.into_split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });

    let dir = std::env::temp_dir().join(format!("sethu-ws-{}", Uuid::new_v4()));
    let state = AppState::new(Config::for_tests(dir.clone())).expect("state");
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
        .json(&json!({"email": "ws@example.com", "password": "websocket-password"}))
        .send()
        .await
        .expect("register");
    assert_eq!(register.status(), reqwest::StatusCode::CREATED);
    let cookie = register
        .headers()
        .get("set-cookie")
        .expect("set-cookie")
        .to_str()
        .expect("cookie text")
        .split(';')
        .next()
        .expect("cookie pair")
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
            "name": "Fake VNC",
            "kind": "macos",
            "target": device_addr.to_string(),
        }))
        .send()
        .await
        .expect("create host")
        .json()
        .await
        .expect("host json");
    let host_id = created["id"].as_str().expect("host id").to_string();

    let mut request = format!("ws://{addr}/ws/vnc/{host_id}")
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

    let banner = Bytes::from_static(b"RFB 003.008\n");
    socket
        .send(Message::Binary(banner.clone()))
        .await
        .expect("send");

    let echoed = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("timeout")
        .expect("message")
        .expect("ok message");
    match echoed {
        Message::Binary(bytes) => assert_eq!(bytes, banner),
        other => panic!("unexpected message: {other:?}"),
    }

    let _ = socket.close(None).await;
    server.abort();
    let _ = std::fs::remove_dir_all(dir);
}
