//! Cross-network device presence and file transfer.
//!
//! Every open Sethu browser tab announces itself with a stable, per-browser
//! device id. A file can be sent to another device only while *both* devices
//! are online (they heartbeated within the presence TTL). The transfer then
//! waits for the receiving device to accept it, and only a delivered transfer
//! can be downloaded.
//!
//! This is what makes file exchange work when the devices are not on the same
//! Wi-Fi network: each browser only needs to reach the Sethu server. The two
//! devices never talk to each other directly, and nothing is relayed
//! peer-to-peer.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, files, state::AppState, store::StoredFile};

/// Completed transfers are kept this long so both devices can see the result.
const TRANSFER_RETENTION_SECS: i64 = 24 * 60 * 60;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/presence", get(list_presence).post(heartbeat))
        .route("/api/presence/{device_id}", delete(go_offline))
        .route("/api/transfers", get(list_transfers))
        .route("/api/transfers/{id}/accept", post(accept_transfer))
        .route("/api/transfers/{id}/cancel", post(cancel_transfer))
        .route("/api/transfers/{id}/download", get(download_transfer))
        .route("/api/files/{id}/transfer", post(create_transfer))
}

// ----- presence ----------------------------------------------------------

#[derive(Debug, Clone)]
struct Peer {
    name: String,
    last_seen: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerView {
    pub id: Uuid,
    pub name: String,
    pub last_seen: i64,
}

/// In-memory presence. Restarting the server simply marks every device offline,
/// which is the correct outcome: a tab that is still open re-announces itself
/// on its next heartbeat.
#[derive(Default)]
pub struct PresenceStore {
    peers: Mutex<HashMap<(Uuid, Uuid), Peer>>,
}

impl PresenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn prune(map: &mut HashMap<(Uuid, Uuid), Peer>, now: i64, ttl: i64) {
        map.retain(|_, peer| now.saturating_sub(peer.last_seen) <= ttl);
    }

    fn snapshot(
        map: &HashMap<(Uuid, Uuid), Peer>,
        user_id: Uuid,
        now: i64,
        ttl: i64,
    ) -> Vec<PeerView> {
        let mut peers: Vec<PeerView> = map
            .iter()
            .filter(|((owner, _), peer)| {
                *owner == user_id && now.saturating_sub(peer.last_seen) <= ttl
            })
            .map(|((_, id), peer)| PeerView {
                id: *id,
                name: peer.name.clone(),
                last_seen: peer.last_seen,
            })
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        peers
    }

    pub fn heartbeat(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        name: String,
        ttl: Duration,
    ) -> Vec<PeerView> {
        let now = now_unix();
        let ttl = ttl.as_secs().min(i64::MAX as u64) as i64;
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut map, now, ttl);
        map.insert(
            (user_id, device_id),
            Peer {
                name,
                last_seen: now,
            },
        );
        Self::snapshot(&map, user_id, now, ttl)
    }

    pub fn go_offline(&self, user_id: Uuid, device_id: Uuid) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&(user_id, device_id));
    }

    /// Returns the peer only if it heartbeated within the TTL.
    pub fn peer(&self, user_id: Uuid, device_id: Uuid, ttl: Duration) -> Option<PeerView> {
        let now = now_unix();
        let ttl = ttl.as_secs().min(i64::MAX as u64) as i64;
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut map, now, ttl);
        map.get(&(user_id, device_id)).map(|peer| PeerView {
            id: device_id,
            name: peer.name.clone(),
            last_seen: peer.last_seen,
        })
    }

    pub fn list(&self, user_id: Uuid, ttl: Duration) -> Vec<PeerView> {
        let now = now_unix();
        let ttl = ttl.as_secs().min(i64::MAX as u64) as i64;
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut map, now, ttl);
        Self::snapshot(&map, user_id, now, ttl)
    }
}

#[derive(Deserialize)]
struct PresenceRequest {
    device_id: Uuid,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Serialize)]
struct PresenceView {
    ttl_seconds: u64,
    peers: Vec<PeerView>,
}

async fn heartbeat(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<PresenceRequest>,
) -> Result<Json<PresenceView>, AppError> {
    if body.device_id.is_nil() {
        return Err(AppError::BadRequest("invalid device id".into()));
    }
    let peers = state.presence.heartbeat(
        user.user.id,
        body.device_id,
        clean_device_name(body.name.as_deref()),
        state.cfg.presence_ttl,
    );
    Ok(Json(PresenceView {
        ttl_seconds: state.cfg.presence_ttl.as_secs(),
        peers,
    }))
}

async fn list_presence(State(state): State<AppState>, user: AuthUser) -> Json<PresenceView> {
    Json(PresenceView {
        ttl_seconds: state.cfg.presence_ttl.as_secs(),
        peers: state.presence.list(user.user.id, state.cfg.presence_ttl),
    })
}

async fn go_offline(
    State(state): State<AppState>,
    user: AuthUser,
    Path(device_id): Path<Uuid>,
) -> StatusCode {
    state.presence.go_offline(user.user.id, device_id);
    StatusCode::NO_CONTENT
}

pub fn clean_device_name(raw: Option<&str>) -> String {
    let name = raw.unwrap_or_default().trim();
    let cleaned: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "This device".into()
    } else {
        cleaned.into()
    }
}

// ----- transfers ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    Delivered,
    Canceled,
    Expired,
}

#[derive(Debug, Clone)]
struct Transfer {
    id: Uuid,
    owner_id: Uuid,
    file: StoredFile,
    from: PeerView,
    to: PeerView,
    status: TransferStatus,
    created_at: i64,
    expires_at: i64,
    delivered_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferView {
    pub id: Uuid,
    pub file_id: Uuid,
    pub file_name: String,
    pub size: u64,
    pub mime: String,
    pub from: DeviceRef,
    pub to: DeviceRef,
    pub status: TransferStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub delivered_at: Option<i64>,
    /// Present once the receiving device has accepted the transfer.
    pub download_url: Option<String>,
}

#[derive(Default)]
pub struct TransferStore {
    transfers: Mutex<HashMap<Uuid, Transfer>>,
}

impl TransferStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn view(transfer: &Transfer) -> TransferView {
        TransferView {
            id: transfer.id,
            file_id: transfer.file.id,
            file_name: transfer.file.name.clone(),
            size: transfer.file.size,
            mime: transfer.file.mime.clone(),
            from: DeviceRef {
                id: transfer.from.id,
                name: transfer.from.name.clone(),
            },
            to: DeviceRef {
                id: transfer.to.id,
                name: transfer.to.name.clone(),
            },
            status: transfer.status,
            created_at: transfer.created_at,
            expires_at: transfer.expires_at,
            delivered_at: transfer.delivered_at,
            download_url: (transfer.status == TransferStatus::Delivered)
                .then(|| format!("/api/transfers/{}/download", transfer.id)),
        }
    }

    pub fn create(
        &self,
        owner_id: Uuid,
        file: StoredFile,
        from: PeerView,
        to: PeerView,
        ttl: Duration,
    ) -> TransferView {
        let now = now_unix();
        let transfer = Transfer {
            id: Uuid::new_v4(),
            owner_id,
            file,
            from,
            to,
            status: TransferStatus::Pending,
            created_at: now,
            expires_at: now.saturating_add(ttl.as_secs().min(i64::MAX as u64) as i64),
            delivered_at: None,
        };
        let view = Self::view(&transfer);
        let mut map = self.transfers.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(transfer.id, transfer);
        view
    }

    pub fn list(&self, owner_id: Uuid) -> Vec<TransferView> {
        let now = now_unix();
        let mut map = self.transfers.lock().unwrap_or_else(|e| e.into_inner());
        for transfer in map.values_mut() {
            if transfer.status == TransferStatus::Pending && transfer.expires_at <= now {
                transfer.status = TransferStatus::Expired;
            }
        }
        map.retain(|_, transfer| {
            transfer.created_at >= now.saturating_sub(TRANSFER_RETENTION_SECS)
        });
        let mut views: Vec<TransferView> = map
            .values()
            .filter(|transfer| transfer.owner_id == owner_id)
            .map(Self::view)
            .collect();
        views.sort_by_key(|view| std::cmp::Reverse(view.created_at));
        views
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Transfer>> {
        self.transfers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn find(map: &HashMap<Uuid, Transfer>, id: Uuid, owner_id: Uuid) -> Result<Transfer, AppError> {
        map.get(&id)
            .filter(|transfer| transfer.owner_id == owner_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("transfer not found".into()))
    }

    pub fn accept(
        &self,
        id: Uuid,
        owner_id: Uuid,
        device_id: Uuid,
    ) -> Result<TransferView, AppError> {
        let mut map = self.lock();
        let transfer = Self::find(&map, id, owner_id)?;
        if transfer.to.id != device_id {
            return Err(AppError::Forbidden(
                "this transfer belongs to a different device".into(),
            ));
        }
        if transfer.status != TransferStatus::Pending {
            return Err(AppError::Conflict(
                "this transfer is no longer waiting to be received".into(),
            ));
        }
        if transfer.expires_at <= now_unix() {
            if let Some(stored) = map.get_mut(&id) {
                stored.status = TransferStatus::Expired;
            }
            return Err(AppError::Conflict(
                "this transfer expired before the device came online".into(),
            ));
        }
        let stored = map
            .get_mut(&id)
            .ok_or_else(|| AppError::NotFound("transfer not found".into()))?;
        stored.status = TransferStatus::Delivered;
        stored.delivered_at = Some(now_unix());
        Ok(Self::view(stored))
    }

    pub fn cancel(
        &self,
        id: Uuid,
        owner_id: Uuid,
        device_id: Uuid,
    ) -> Result<TransferView, AppError> {
        let mut map = self.lock();
        let transfer = Self::find(&map, id, owner_id)?;
        if transfer.from.id != device_id {
            return Err(AppError::Forbidden(
                "only the sending device can cancel this transfer".into(),
            ));
        }
        if transfer.status != TransferStatus::Pending {
            return Err(AppError::Conflict(
                "this transfer is already finished".into(),
            ));
        }
        let stored = map
            .get_mut(&id)
            .ok_or_else(|| AppError::NotFound("transfer not found".into()))?;
        stored.status = TransferStatus::Canceled;
        Ok(Self::view(stored))
    }

    /// A delivered transfer can be downloaded by its workspace (same user).
    pub fn delivered(&self, id: Uuid, owner_id: Uuid) -> Result<StoredFile, AppError> {
        let map = self.lock();
        let transfer = Self::find(&map, id, owner_id)?;
        if transfer.status != TransferStatus::Delivered {
            return Err(AppError::Forbidden(
                "receive this transfer before downloading it".into(),
            ));
        }
        Ok(transfer.file)
    }
}

#[derive(Deserialize)]
struct TransferRequest {
    from_device: Uuid,
    to_device: Uuid,
}

#[derive(Deserialize)]
struct DeviceRequest {
    device_id: Uuid,
}

async fn create_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(file_id): Path<Uuid>,
    Json(body): Json<TransferRequest>,
) -> Result<(StatusCode, Json<TransferView>), AppError> {
    if body.from_device == body.to_device {
        return Err(AppError::BadRequest(
            "pick another device to send the file to".into(),
        ));
    }
    let ttl = state.cfg.presence_ttl;
    let from = state
        .presence
        .peer(user.user.id, body.from_device, ttl)
        .ok_or_else(|| {
            AppError::Conflict(
                "this device went offline. Keep this Sethu tab open and try again.".into(),
            )
        })?;
    let to = state
        .presence
        .peer(user.user.id, body.to_device, ttl)
        .ok_or_else(|| {
            AppError::Conflict(
                "the other device is offline. Open Sethu there and sign in with the same username."
                    .into(),
            )
        })?;
    let file = state
        .store
        .file(file_id, user.user.id)
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;
    if !files::file_path(&state, &file).is_file() {
        return Err(AppError::NotFound("file is missing from disk".into()));
    }

    let view = state
        .transfers
        .create(user.user.id, file, from, to, state.cfg.transfer_ttl);
    tracing::info!(
        user_id = %user.user.id,
        transfer_id = %view.id,
        from = %view.from.name,
        to = %view.to.name,
        "device transfer queued"
    );
    Ok((StatusCode::CREATED, Json(view)))
}

async fn list_transfers(State(state): State<AppState>, user: AuthUser) -> Json<Vec<TransferView>> {
    Json(state.transfers.list(user.user.id))
}

async fn accept_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DeviceRequest>,
) -> Result<Json<TransferView>, AppError> {
    if state
        .presence
        .peer(user.user.id, body.device_id, state.cfg.presence_ttl)
        .is_none()
    {
        return Err(AppError::Conflict(
            "this device is offline. Reopen Sethu here to receive the file.".into(),
        ));
    }
    let view = state.transfers.accept(id, user.user.id, body.device_id)?;
    tracing::info!(
        user_id = %user.user.id,
        transfer_id = %id,
        "device transfer accepted"
    );
    Ok(Json(view))
}

async fn cancel_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DeviceRequest>,
) -> Result<Json<TransferView>, AppError> {
    Ok(Json(state.transfers.cancel(
        id,
        user.user.id,
        body.device_id,
    )?))
}

async fn download_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let transfer = state.transfers.delivered(id, user.user.id)?;
    let file = state
        .store
        .file(transfer.id, user.user.id)
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;
    files::stream_file(&state, &file).await
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(name: &str) -> PeerView {
        PeerView {
            id: Uuid::new_v4(),
            name: name.into(),
            last_seen: now_unix(),
        }
    }

    fn file(owner: Uuid) -> StoredFile {
        StoredFile {
            id: Uuid::new_v4(),
            owner_id: owner,
            name: "report.pdf".into(),
            disk_name: "disk".into(),
            size: 42,
            mime: "application/pdf".into(),
            created_at: now_unix(),
        }
    }

    #[test]
    fn presence_marks_devices_online_and_offline() {
        let store = PresenceStore::new();
        let user = Uuid::new_v4();
        let phone = Uuid::new_v4();
        let mac = Uuid::new_v4();
        store.heartbeat(user, phone, "iPhone".into(), Duration::from_secs(45));
        store.heartbeat(user, mac, "MacBook".into(), Duration::from_secs(45));

        assert!(store.peer(user, phone, Duration::from_secs(45)).is_some());
        assert_eq!(store.list(user, Duration::from_secs(45)).len(), 2);

        store.go_offline(user, phone);
        assert!(store.peer(user, phone, Duration::from_secs(45)).is_none());
        assert_eq!(store.list(user, Duration::from_secs(45)).len(), 1);
    }

    #[test]
    fn presence_is_scoped_to_one_workspace() {
        let store = PresenceStore::new();
        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let device = Uuid::new_v4();
        store.heartbeat(alice, device, "Alice Mac".into(), Duration::from_secs(45));
        assert!(store.peer(bob, device, Duration::from_secs(45)).is_none());
        assert!(store.list(bob, Duration::from_secs(45)).is_empty());
    }

    #[test]
    fn expired_presence_is_dropped() {
        let store = PresenceStore::new();
        let user = Uuid::new_v4();
        let device = Uuid::new_v4();
        // A zero TTL means "only the current second", so a tiny sleep expires.
        store.heartbeat(user, device, "Old phone".into(), Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(1_100));
        assert!(store.peer(user, device, Duration::from_secs(0)).is_none());
    }

    #[test]
    fn transfers_need_the_recipient_to_accept() {
        let store = TransferStore::new();
        let owner = Uuid::new_v4();
        let from = peer("MacBook");
        let to = peer("iPhone");
        let created = store.create(
            owner,
            file(owner),
            from.clone(),
            to.clone(),
            Duration::from_secs(600),
        );
        assert_eq!(created.status, TransferStatus::Pending);
        assert!(created.download_url.is_none());
        assert_eq!(store.list(owner).len(), 1);

        let wrong = store.accept(created.id, owner, Uuid::new_v4());
        assert!(wrong.is_err());

        let delivered = store.accept(created.id, owner, to.id).expect("accept");
        assert_eq!(delivered.status, TransferStatus::Delivered);
        assert!(delivered.download_url.is_some());
        assert!(store.delivered(created.id, owner).is_ok());

        // Accepting twice is refused.
        assert!(store.accept(created.id, owner, to.id).is_err());
    }

    #[test]
    fn only_the_sender_can_cancel() {
        let store = TransferStore::new();
        let owner = Uuid::new_v4();
        let from = peer("MacBook");
        let to = peer("Android");
        let created = store.create(
            owner,
            file(owner),
            from.clone(),
            to.clone(),
            Duration::from_secs(600),
        );
        assert!(store.cancel(created.id, owner, to.id).is_err());
        let canceled = store.cancel(created.id, owner, from.id).expect("cancel");
        assert_eq!(canceled.status, TransferStatus::Canceled);
    }

    #[test]
    fn transfers_are_scoped_to_one_workspace() {
        let store = TransferStore::new();
        let owner = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let created = store.create(
            owner,
            file(owner),
            peer("Mac"),
            peer("Phone"),
            Duration::from_secs(600),
        );
        assert!(store.list(stranger).is_empty());
        assert!(store.delivered(created.id, stranger).is_err());
    }

    #[test]
    fn device_names_are_sanitized() {
        assert_eq!(clean_device_name(None), "This device");
        assert_eq!(clean_device_name(Some("   ")), "This device");
        assert_eq!(clean_device_name(Some("  iPhone\n15 ")), "iPhone15");
        assert!(clean_device_name(Some(&"x".repeat(200))).chars().count() <= 40);
    }
}
