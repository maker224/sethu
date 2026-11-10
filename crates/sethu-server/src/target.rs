//! Validation and normalisation of user supplied device addresses.
//!
//! Hosts are added by authenticated users, so this is about catching typos and
//! keeping the stored format canonical, not about blocking SSRF (a user can
//! already reach their own LAN directly).

use crate::error::AppError;

const DEFAULT_VNC_PORT: u16 = 5900;

/// Accepts `host`, `host:port`, `vnc://host:port` and bracketed IPv6.
pub fn normalize_vnc(raw: &str) -> Result<String, AppError> {
    let value = raw.trim();
    let value = value
        .strip_prefix("vnc://")
        .or_else(|| value.strip_prefix("rfb://"))
        .unwrap_or(value);
    if value.is_empty()
        || value.contains("://")
        || value.contains('/')
        || value.contains('@')
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError::BadRequest(
            "macOS target must be a hostname or IP, optionally with a port".into(),
        ));
    }

    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| AppError::BadRequest("unclosed IPv6 address".into()))?;
        let port = match tail {
            "" => DEFAULT_VNC_PORT,
            tail => tail
                .strip_prefix(':')
                .and_then(|port| port.parse::<u16>().ok())
                .ok_or_else(|| AppError::BadRequest("invalid port".into()))?,
        };
        (host.to_string(), port)
    } else if value.matches(':').count() > 1 {
        return Err(AppError::BadRequest(
            "wrap IPv6 addresses in brackets, for example [fd00::1]:5900".into(),
        ));
    } else if let Some((host, port)) = value.rsplit_once(':') {
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| AppError::BadRequest("invalid port".into()))?,
        )
    } else {
        (value.to_string(), DEFAULT_VNC_PORT)
    };

    if host.is_empty()
        || host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(AppError::BadRequest("invalid hostname".into()));
    }
    if port == 0 {
        return Err(AppError::BadRequest(
            "port must be between 1 and 65535".into(),
        ));
    }

    let display_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    Ok(format!("{display_host}:{port}"))
}

/// Accepts the base URL of a WebDriverAgent server, e.g. `http://192.168.1.9:8100`.
pub fn normalize_wda(raw: &str) -> Result<String, AppError> {
    let value = raw.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(value)
        .map_err(|_| AppError::BadRequest("iPhone target must be an http(s) URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadRequest(
            "iPhone target must start with http:// or https://".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(AppError::BadRequest("iPhone target has no host".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadRequest(
            "credentials are not allowed in the target URL".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::BadRequest(
            "target URL must not contain a query or fragment".into(),
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vnc_defaults_and_ports() {
        assert_eq!(normalize_vnc("mac.local").unwrap(), "mac.local:5900");
        assert_eq!(normalize_vnc("10.0.0.5:5901").unwrap(), "10.0.0.5:5901");
        assert_eq!(normalize_vnc("vnc://10.0.0.5").unwrap(), "10.0.0.5:5900");
        assert_eq!(normalize_vnc("[fd00::1]:5900").unwrap(), "[fd00::1]:5900");
    }

    #[test]
    fn vnc_rejects_junk() {
        assert!(normalize_vnc("").is_err());
        assert!(normalize_vnc("http://x").is_err());
        assert!(normalize_vnc("host:0").is_err());
        assert!(normalize_vnc("host:99999").is_err());
        assert!(normalize_vnc("fd00::1").is_err());
    }

    #[test]
    fn wda_urls_are_canonical() {
        assert_eq!(
            normalize_wda(" http://192.168.1.9:8100/ ").unwrap(),
            "http://192.168.1.9:8100"
        );
        assert!(normalize_wda("192.168.1.9:8100").is_err());
        assert!(normalize_wda("ftp://x").is_err());
        assert!(normalize_wda("http://user:pass@x").is_err());
    }
}
