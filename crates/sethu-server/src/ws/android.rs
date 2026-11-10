//! Android mirroring over `adb`.
//!
//! Like the iPhone bridge this polls PNG screenshots, but Android delivers
//! pixels and `input tap` accepts pixels, so no point conversion is needed.

use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
};
use bytes::Bytes;
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    adb::AdbClient, auth::AuthUser, error::AppError, state::AppState, store::HostKind,
    util::png_size, ws::host_for,
};

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let host = host_for(&state, &user, id, &[HostKind::Android]).await?;
    let permit = state
        .ws_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests)?;
    let adb = AdbClient::new(&state.cfg.adb_path, &host.target, state.cfg.adb_timeout);

    // Fail the upgrade with a useful message instead of opening a dead socket.
    let first_frame = adb
        .screenshot()
        .await
        .map_err(|err| AppError::BadGateway(format!("Android device is not reachable: {err}")))?;
    let Some(size) = png_size(&first_frame) else {
        return Err(AppError::BadGateway(
            "adb returned something that is not a PNG screenshot".into(),
        ));
    };

    tracing::info!(
        user_id = %user.user.id,
        host_id = %host.id,
        serial = %host.target,
        width = size.0,
        height = size.1,
        "android session opened"
    );

    Ok(ws
        .max_frame_size(16 * 1024 * 1024)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            if let Err(err) = bridge(socket, adb, first_frame, state.cfg.android_fps).await {
                tracing::info!(error = %err, "android session ended");
            }
        }))
}

enum Frame {
    Png { bytes: Bytes, size: (u32, u32) },
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Tap {
        x: f64,
        y: f64,
    },
    Swipe {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[serde(default)]
        duration: Option<f64>,
    },
    Keys {
        text: String,
    },
    Button {
        name: String,
    },
    SetFps {
        value: u8,
    },
    Ping,
}

async fn bridge(socket: WebSocket, adb: AdbClient, first: Bytes, fps: u8) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    let fps = Arc::new(AtomicU8::new(fps.clamp(1, 15)));
    let (frame_tx, mut frame_rx) = mpsc::channel::<Frame>(2);
    let producer = tokio::spawn(producer_loop(adb.clone(), frame_tx, fps.clone()));

    let mut screen = png_size(&first).unwrap_or((0, 0));
    send_json(
        &mut sink,
        json!({
            "type": "ready",
            "width": screen.0,
            "height": screen.1,
            "scale": 1,
            "fps": fps.load(Ordering::Relaxed),
        }),
    )
    .await?;
    sink.send(Message::Binary(first)).await?;

    loop {
        tokio::select! {
            frame = frame_rx.recv() => {
                match frame {
                    Some(Frame::Png { bytes, size }) => {
                        if size != screen {
                            screen = size;
                            send_json(&mut sink, json!({
                                "type": "size",
                                "width": screen.0,
                                "height": screen.1,
                            })).await?;
                        }
                        sink.send(Message::Binary(bytes)).await?;
                    }
                    Some(Frame::Error(message)) => {
                        send_json(&mut sink, json!({ "type": "error", "message": message })).await?;
                    }
                    None => break,
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_command(&mut sink, &adb, text.as_str(), screen, &fps).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(err)) => {
                        tracing::debug!(%err, "android websocket error");
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    producer.abort();
    Ok(())
}

async fn handle_command(
    sink: &mut SplitSink<WebSocket, Message>,
    adb: &AdbClient,
    raw: &str,
    screen: (u32, u32),
    fps: &Arc<AtomicU8>,
) -> Result<()> {
    let command: ClientMsg = match serde_json::from_str(raw) {
        Ok(command) => command,
        Err(err) => {
            send_json(
                sink,
                json!({ "type": "error", "message": format!("invalid command: {err}") }),
            )
            .await?;
            return Ok(());
        }
    };

    let action: Result<(), AppError> = match command {
        ClientMsg::Ping => {
            send_json(sink, json!({ "type": "pong" })).await?;
            return Ok(());
        }
        ClientMsg::SetFps { value } => {
            let value = value.clamp(2, 15);
            fps.store(value, Ordering::Relaxed);
            send_json(sink, json!({ "type": "fps", "value": value })).await?;
            return Ok(());
        }
        ClientMsg::Tap { x, y } => {
            if let Err(message) = check_bounds(screen, &[(x, y)]) {
                send_json(sink, json!({ "type": "error", "message": message })).await?;
                return Ok(());
            }
            adb.tap(x.round() as u32, y.round() as u32).await
        }
        ClientMsg::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration,
        } => {
            if let Err(message) = check_bounds(screen, &[(x1, y1), (x2, y2)]) {
                send_json(sink, json!({ "type": "error", "message": message })).await?;
                return Ok(());
            }
            let duration_ms = (duration.unwrap_or(0.25) * 1_000.0).round() as u32;
            adb.swipe(
                x1.round() as u32,
                y1.round() as u32,
                x2.round() as u32,
                y2.round() as u32,
                duration_ms,
            )
            .await
        }
        ClientMsg::Keys { text } => adb.type_text(&text).await,
        ClientMsg::Button { name } => adb.press(&name).await,
    };

    if let Err(err) = action {
        send_json(sink, json!({ "type": "error", "message": err.to_string() })).await?;
    }
    Ok(())
}

fn check_bounds(screen: (u32, u32), points: &[(f64, f64)]) -> Result<(), String> {
    if screen.0 == 0 || screen.1 == 0 {
        return Ok(());
    }
    for &(x, y) in points {
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x > f64::from(screen.0)
            || y > f64::from(screen.1)
        {
            return Err("touch coordinates are outside the screen".into());
        }
    }
    Ok(())
}

async fn send_json(sink: &mut SplitSink<WebSocket, Message>, value: Value) -> Result<()> {
    sink.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}

async fn producer_loop(adb: AdbClient, tx: mpsc::Sender<Frame>, fps: Arc<AtomicU8>) {
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        match adb.screenshot().await {
            Ok(bytes) => match png_size(&bytes) {
                Some(size) => {
                    failures = 0;
                    if tx.send(Frame::Png { bytes, size }).await.is_err() {
                        return;
                    }
                }
                None => failures += 1,
            },
            Err(err) => {
                failures += 1;
                if failures == 1 || failures % 10 == 0 {
                    tracing::warn!(error = %err, "android screencap failed");
                    if tx.send(Frame::Error(err.to_string())).await.is_err() {
                        return;
                    }
                }
            }
        }

        let interval = Duration::from_millis(1_000 / u64::from(fps.load(Ordering::Relaxed).max(1)));
        if let Some(rest) = interval.checked_sub(started.elapsed()) {
            tokio::time::sleep(rest).await;
        }
        if failures > 0 {
            tokio::time::sleep(Duration::from_millis(u64::from(failures.min(10)) * 200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_enforced() {
        assert!(check_bounds((100, 200), &[(0.0, 0.0), (100.0, 200.0)]).is_ok());
        assert!(check_bounds((100, 200), &[(101.0, 10.0)]).is_err());
        assert!(check_bounds((100, 200), &[(f64::NAN, 10.0)]).is_err());
    }

    #[test]
    fn commands_deserialize() {
        let tap: ClientMsg = serde_json::from_str(r#"{"type":"tap","x":1,"y":2}"#).unwrap();
        assert!(matches!(tap, ClientMsg::Tap { .. }));
        let swipe: ClientMsg =
            serde_json::from_str(r#"{"type":"swipe","x1":1,"y1":2,"x2":3,"y2":4}"#).unwrap();
        assert!(matches!(swipe, ClientMsg::Swipe { .. }));
    }
}
