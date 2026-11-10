//! Frontend assets are compiled into the binary so deployment is a single
//! executable. `SETHU_WEB_DIR` can override them at runtime for development.

macro_rules! asset {
    ($path:literal, $mime:literal) => {
        (
            $path,
            $mime,
            include_bytes!(concat!("../../../web/", $path)) as &[u8],
        )
    };
}

/// Look up an embedded asset. Returns `(mime, bytes)`.
pub fn lookup(path: &str) -> Option<(&'static str, &'static [u8])> {
    let (_, mime, bytes) = match path {
        "/" | "/index.html" => asset!("index.html", "text/html; charset=utf-8"),
        "/app.js" => asset!("app.js", "text/javascript; charset=utf-8"),
        "/styles.css" => asset!("styles.css", "text/css; charset=utf-8"),
        "/viewer.html" => asset!("viewer.html", "text/html; charset=utf-8"),
        "/viewer.js" => asset!("viewer.js", "text/javascript; charset=utf-8"),
        "/viewer-vnc.js" => asset!("viewer-vnc.js", "text/javascript; charset=utf-8"),
        "/viewer-ios.js" => asset!("viewer-ios.js", "text/javascript; charset=utf-8"),
        "/favicon.svg" => asset!("favicon.svg", "image/svg+xml"),
        "/manifest.webmanifest" => asset!("manifest.webmanifest", "application/manifest+json"),
        _ => return None,
    };
    Some((mime, bytes))
}
