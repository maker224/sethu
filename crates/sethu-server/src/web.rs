//! Static file serving: embedded assets by default, disk override for dev.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{assets, state::AppState};

pub async fn serve(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().to_string();

    if path.starts_with("/api/") || path.starts_with("/ws/") {
        return not_found();
    }

    // Disk mode (development): serve files from SETHU_WEB_DIR.
    if let Some(dir) = &state.cfg.web_dir {
        let relative = if path == "/" {
            "index.html".to_string()
        } else {
            path.trim_start_matches('/').to_string()
        };
        if relative
            .split('/')
            .all(|part| part != ".." && !part.contains('\\'))
        {
            let file = dir.join(&relative);
            if let Ok(bytes) = tokio::fs::read(&file).await {
                let mime = mime_for(&relative);
                return with_cache(mime, bytes, &path, false);
            }
        }
    }

    match assets::lookup(&path) {
        Some((mime, bytes)) => with_cache(mime, bytes.to_vec(), &path, false),
        None => {
            // Optional third-party assets (vendored noVNC) live outside the
            // binary so they can be dropped in without a rebuild.
            if path.starts_with("/vendor/") {
                if let Some(response) = read_vendor(&state, &path).await {
                    return response;
                }
            }
            not_found()
        }
    }
}

async fn read_vendor(state: &AppState, path: &str) -> Option<Response> {
    let relative = path.trim_start_matches('/');
    if !relative
        .split('/')
        .all(|part| !part.is_empty() && part != ".." && !part.contains('\\'))
    {
        return None;
    }
    let bytes = tokio::fs::read(state.cfg.vendor_dir.join(relative))
        .await
        .ok()?;
    Some(with_cache(mime_for(relative), bytes, path, false))
}

fn with_cache(mime: &str, bytes: Vec<u8>, path: &str, immutable: bool) -> Response {
    let etag = format!("\"sethu-{}-{path}\"", env!("CARGO_PKG_VERSION"));
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache)
        .header(header::ETAG, etag)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "not found",
    )
        .into_response()
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webmanifest" => "application/manifest+json",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
