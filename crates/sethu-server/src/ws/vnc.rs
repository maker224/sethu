//! VNC (RFB) passthrough.
//!
//! The browser speaks RFB directly (via noVNC) through this WebSocket; the
//! server is a byte pump and never sees credentials or framebuffer contents in
//! a form it understands. macOS authenticates with the VNC password configured
//! in System Settings, which keeps secrets out of Sethu entirely.

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, state::AppState, store::HostKind, ws::host_for};

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let host = host_for(&state, &user, id, &[HostKind::Macos, HostKind::Windows]).await?;
    let permit = state
        .ws_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests)?;

    // Connect before upgrading so the browser gets an HTTP error instead of a
    // silently dead socket.
    let stream = tokio::time::timeout(
        state.cfg.vnc_connect_timeout,
        TcpStream::connect(&host.target),
    )
    .await
    .map_err(|_| AppError::BadGateway(format!("timed out connecting to {}", host.target)))?
    .map_err(|err| AppError::BadGateway(format!("could not reach {}: {err}", host.target)))?;
    let _ = stream.set_nodelay(true);

    tracing::info!(user_id = %user.user.id, host_id = %host.id, target = %host.target, "vnc session opened");
    Ok(ws
        .protocols(["binary"])
        .max_frame_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let target = host.target.clone();
            if let Err(err) = bridge(socket, stream).await {
                tracing::info!(%target, error = %err, "vnc session ended");
            }
        }))
}

async fn bridge(socket: WebSocket, stream: TcpStream) -> Result<()> {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (mut device_read, mut device_write) = stream.into_split();

    let client_to_device = async {
        while let Some(message) = ws_stream.next().await {
            match message {
                Ok(Message::Binary(bytes)) => {
                    device_write.write_all(&bytes).await?;
                }
                Ok(Message::Text(text)) => {
                    // Reserved for future control messages; RFB itself only
                    // uses binary frames.
                    if text.as_str() == "ping" {
                        continue;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = device_write.shutdown().await;
        Ok::<(), anyhow::Error>(())
    };

    let device_to_client = async {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = device_read
                .read(&mut buffer)
                .await
                .context("reading from VNC server")?;
            if read == 0 {
                break;
            }
            ws_sink
                .send(Message::Binary(Bytes::copy_from_slice(&buffer[..read])))
                .await
                .context("sending to browser")?;
        }
        Ok::<(), anyhow::Error>(())
    };

    tokio::select! {
        result = client_to_device => result?,
        result = device_to_client => result?,
    }
    Ok(())
}
