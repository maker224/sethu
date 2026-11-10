//! iPhone mirroring over WebDriverAgent.
//!
//! WDA only exposes screenshots and input injection, so this bridge polls PNG
//! frames at a configurable rate and translates browser gestures into WDA
//! commands. Audio and DRM-protected video are not available; see the README.

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
    auth::AuthUser, error::AppError, state::AppState, store::HostKind, util::png_size,
    wda::WdaClient, ws::host_for,
};

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let host = host_for(&state, &user, id, HostKind::Ios).await?;
    let permit = state
        .ws_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests)?;
    let client = WdaClient::from_target(&host.target, state.http.clone(), state.cfg.wda_timeout)?;

    // Fail the handshake (instead of opening a dead socket) when the phone or
    // WebDriverAgent is unreachable.
    let first_frame = client.screenshot().await.map_err(|err| {
        AppError::BadGateway(format!("iPhone is not reachable via WebDriverAgent: {err}"))
    })?;
    let Some(size) = png_size(&first_frame) else {
        return Err(AppError::BadGateway(
            "WebDriverAgent returned something that is not a PNG screenshot".into(),
        ));
    };

    // Screenshots are in physical pixels while WDA taps use logical points.
    // Ask WDA for the point size so calculations stay exact across devices.
    let scale = match client.screen_points().await {
        Some((width, _)) if width > 0.0 => (f64::from(size.0) / width).clamp(1.0, 4.0),
        _ => 1.0,
    };

    tracing::info!(
        user_id = %user.user.id,
        host_id = %host.id,
        width = size.0,
        height = size.1,
        scale,
        "iphone session opened"
    );

    Ok(ws
        .max_frame_size(16 * 1024 * 1024)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            if let Err(err) = bridge(socket, client, first_frame, state.cfg.ios_fps, scale).await {
                tracing::info!(error = %err, "iphone session ended");
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

async fn bridge(
    socket: WebSocket,
    client: WdaClient,
    first: Bytes,
    fps: u8,
    scale: f64,
) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    let fps = Arc::new(AtomicU8::new(fps.clamp(1, 15)));
    let (frame_tx, mut frame_rx) = mpsc::channel::<Frame>(2);
    let producer = tokio::spawn(producer_loop(client.clone(), frame_tx, fps.clone()));

    let mut screen = png_size(&first).unwrap_or((0, 0));
    send_json(
        &mut sink,
        json!({
            "type": "ready",
            "width": screen.0,
            "height": screen.1,
            "scale": scale,
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
                        handle_command(&mut sink, &client, text.as_str(), screen, &fps).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(err)) => {
                        tracing::debug!(%err, "iphone websocket error");
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
    client: &WdaClient,
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
            client.tap(x, y).await
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
            client.swipe(x1, y1, x2, y2, duration.unwrap_or(0.25)).await
        }
        ClientMsg::Keys { text } => {
            if text.chars().count() > 2_000 {
                send_json(
                    sink,
                    json!({ "type": "error", "message": "text input is limited to 2000 characters" }),
                )
                .await?;
                return Ok(());
            }
            client.type_text(&text).await
        }
        ClientMsg::Button { name } => client.press(&name).await,
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

async fn producer_loop(client: WdaClient, tx: mpsc::Sender<Frame>, fps: Arc<AtomicU8>) {
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        match client.screenshot().await {
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
                    tracing::warn!(error = %err, "screenshot failed");
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
        // Back off when the phone is struggling instead of hammering WDA.
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
        let fps: ClientMsg = serde_json::from_str(r#"{"type":"set_fps","value":6}"#).unwrap();
        assert!(matches!(fps, ClientMsg::SetFps { value: 6 }));
    }
}
