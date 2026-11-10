//! Shared file workspace.
//!
//! Files are streamed to disk under `data/files/<owner>/<uuid>` and listed via
//! a small JSON API. Any format is accepted. Downloads are always served as
//! attachments so an uploaded HTML/SVG file cannot execute in the Sethu origin.

use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State},
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    adb::AdbClient,
    auth::AuthUser,
    error::AppError,
    state::AppState,
    store::{HostKind, StoredFile},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/files", get(list_files).post(upload_file))
        .route("/api/files/{id}", delete(delete_file))
        .route("/api/files/{id}/download", get(download_file))
        .route("/api/files/{id}/send", post(send_file))
        // Uploads are streamed and capped manually in the handler.
        .layer(DefaultBodyLimit::disable())
}

#[derive(Serialize)]
struct FileView {
    id: Uuid,
    name: String,
    size: u64,
    mime: String,
    created_at: i64,
}

impl From<StoredFile> for FileView {
    fn from(file: StoredFile) -> Self {
        Self {
            id: file.id,
            name: file.name,
            size: file.size,
            mime: file.mime,
            created_at: file.created_at,
        }
    }
}

async fn list_files(State(state): State<AppState>, user: AuthUser) -> Json<Vec<FileView>> {
    Json(
        state
            .store
            .list_files(user.user.id)
            .into_iter()
            .map(FileView::from)
            .collect(),
    )
}

async fn upload_file(
    State(state): State<AppState>,
    user: AuthUser,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<FileView>), AppError> {
    let max = state.cfg.max_file_bytes;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::BadRequest(format!("invalid upload: {err}")))?
    {
        let Some(raw_name) = field.file_name().map(str::to_string) else {
            continue;
        };
        let name = sanitize_filename(&raw_name)
            .ok_or_else(|| AppError::BadRequest("the uploaded file has an invalid name".into()))?;
        let mime = field
            .content_type()
            .filter(|value| value.len() <= 120 && !value.chars().any(char::is_control))
            .map(str::to_string)
            .unwrap_or_else(|| "application/octet-stream".into());

        let dir = files_dir(&state, user.user.id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(AppError::internal)?;
        let disk_name = Uuid::new_v4().to_string();
        let path = dir.join(&disk_name);
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(AppError::internal)?;

        let mut size: u64 = 0;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    size = size.saturating_add(chunk.len() as u64);
                    if size > max {
                        drop(file);
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(AppError::TooLarge(format!(
                            "file exceeds the {} MB limit",
                            max / (1024 * 1024)
                        )));
                    }
                    if let Err(err) = file.write_all(&chunk).await {
                        drop(file);
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(AppError::internal(err));
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(AppError::BadRequest(format!("upload interrupted: {err}")));
                }
            }
        }

        if let Err(err) = file.flush().await {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AppError::internal(err));
        }
        drop(file);

        let record = match state
            .store
            .add_file(user.user.id, name, disk_name, size, mime)
        {
            Ok(record) => record,
            Err(err) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(err.into());
            }
        };
        tracing::info!(
            user_id = %user.user.id,
            file_id = %record.id,
            size,
            "file uploaded"
        );
        return Ok((StatusCode::CREATED, Json(FileView::from(record))));
    }

    Err(AppError::BadRequest("no file was provided".into()))
}

async fn download_file(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Response, AppError> {
    let record = state
        .store
        .file(id, user.user.id)
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;
    let path = file_path(&state, &record);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("file is missing from disk".into()))?;

    let disposition = content_disposition(&record.name);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, record.mime)
        .header(header::CONTENT_LENGTH, record.size)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(AppError::internal)
}

async fn delete_file(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<StatusCode, AppError> {
    let record = state.store.delete_file(id, user.user.id)?;
    let path = file_path(&state, &record);
    let _ = tokio::fs::remove_file(path).await;
    tracing::info!(user_id = %user.user.id, file_id = %id, "file removed");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SendRequest {
    host_id: Uuid,
}

/// Push a shared file into a connected Android device's Downloads folder.
async fn send_file(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
    Json(body): Json<SendRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let record = state
        .store
        .file(id, user.user.id)
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;
    let host = state
        .store
        .host(body.host_id, user.user.id)
        .ok_or_else(|| AppError::NotFound("device not found".into()))?;
    if host.kind != HostKind::Android {
        return Err(AppError::BadRequest(
            "only Android devices can receive files directly".into(),
        ));
    }
    let path = file_path(&state, &record);
    if !path.is_file() {
        return Err(AppError::NotFound("file is missing from disk".into()));
    }

    let adb = AdbClient::new(&state.cfg.adb_path, &host.target, state.cfg.adb_timeout);
    adb.push_download(&path, &record.name).await?;
    tracing::info!(
        user_id = %user.user.id,
        file_id = %record.id,
        host_id = %host.id,
        "file pushed to android"
    );
    Ok(Json(json!({
        "ok": true,
        "message": format!("Sent {} to {} (Downloads)", record.name, host.name),
    })))
}

fn files_dir(state: &AppState, owner: Uuid) -> PathBuf {
    state.cfg.data_dir.join("files").join(owner.to_string())
}

fn file_path(state: &AppState, record: &StoredFile) -> PathBuf {
    // `disk_name` is generated by the server, but take only the basename as a
    // second line of defence against a tampered data file.
    let safe_name = Path::new(&record.disk_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("missing");
    files_dir(state, record.owner_id).join(safe_name)
}

/// Keep the basename, strip control characters and cap the length. Any
/// extension is allowed.
pub fn sanitize_filename(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(180).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        None
    } else {
        Some(cleaned)
    }
}

fn content_disposition(name: &str) -> HeaderValue {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = percent_encode(name);
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(sanitize_filename("report.pdf").unwrap(), "report.pdf");
        assert_eq!(sanitize_filename("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(
            sanitize_filename("C:\\Users\\me\\photo.jpg").unwrap(),
            "photo.jpg"
        );
        assert_eq!(sanitize_filename("  spaced name  ").unwrap(), "spaced name");
        assert!(sanitize_filename("").is_none());
        assert!(sanitize_filename("..").is_none());
        assert!(!sanitize_filename(&"a".repeat(500)).unwrap().is_empty());
        assert_eq!(sanitize_filename(&"a".repeat(500)).unwrap().len(), 180);
    }

    #[test]
    fn disposition_handles_unicode() {
        let value = content_disposition("résumé 2026.pdf");
        let text = value.to_str().unwrap();
        assert!(text.starts_with("attachment;"));
        assert!(text.contains("filename*=UTF-8''"));
        assert!(text.contains("r%C3%A9sum%C3%A9"));
    }

    #[test]
    fn percent_encoding_is_safe() {
        assert_eq!(percent_encode("a b.pdf"), "a%20b.pdf");
        assert_eq!(percent_encode("x\";y"), "x%22%3By");
    }
}
