//! Authentication: password hashing, session cookies, CSRF tokens and the
//! `AuthUser` extractor used by both the JSON API and WebSocket upgrades.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::http::{header, HeaderMap};

use crate::{
    error::AppError,
    state::AppState,
    store::{Session, StoreError, User},
    util::constant_time_eq,
};

pub const SESSION_COOKIE: &str = "sethu_session";
pub const CSRF_HEADER: &str = "x-sethu-csrf";

// Argon2id with the crate defaults (19 MiB, t=2) is intentionally slow; run it
// on the blocking pool so a burst of logins cannot stall the async runtime.
pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| AppError::internal(format!("hash password: {e}")))
    })
    .await
    .map_err(AppError::internal)?
}

pub async fn verify_password(hash: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        let Ok(parsed) = PasswordHash::new(&hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

/// Extract the session token from a `Cookie` header.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE).then(|| value.trim().to_string())
        })
        .filter(|token| !token.is_empty())
}

pub fn session_from_headers(state: &AppState, headers: &HeaderMap) -> Option<(Session, User)> {
    let token = token_from_headers(headers)?;
    let session = state.store.session_by_token(&token)?;
    let user = state.store.user_by_id(session.user_id)?;
    Some((session, user))
}

fn cookie_attributes(state: &AppState) -> String {
    let secure = if state.cfg.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!("Path=/; HttpOnly; SameSite=Strict{secure}")
}

pub fn session_cookie(state: &AppState, token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; {}; Max-Age={}",
        cookie_attributes(state),
        state.cfg.session_ttl.as_secs()
    )
}

pub fn clear_cookie(state: &AppState) -> String {
    format!("{SESSION_COOKIE}=; {}; Max-Age=0", cookie_attributes(state))
}

/// Validate an email without pulling in a regex engine. Good enough to reject
/// obvious garbage; deliverability is out of scope.
pub fn normalize_email(raw: &str) -> Result<String, AppError> {
    let email = raw.trim().to_ascii_lowercase();
    let valid = email.len() >= 3
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
        });
    if valid {
        Ok(email)
    } else {
        Err(AppError::BadRequest("enter a valid email address".into()))
    }
}

pub fn validate_password(password: &str) -> Result<(), AppError> {
    if password.chars().count() < 10 {
        return Err(AppError::BadRequest(
            "password must be at least 10 characters".into(),
        ));
    }
    if password.len() > 1_024 {
        return Err(AppError::BadRequest("password is too long".into()));
    }
    Ok(())
}

/// Authenticated caller, extracted from the session cookie.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub csrf: String,
}

impl axum::extract::FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some((session, user)) = session_from_headers(state, &parts.headers) {
            return Ok(AuthUser {
                user,
                csrf: session.csrf,
            });
        }

        // Open-access mode: every visitor acts as the local workspace owner.
        if state.cfg.open_access {
            let id = state.local_user.ok_or(AppError::Internal)?;
            let user = state
                .store
                .user_by_id(id)
                .ok_or_else(|| AppError::internal("open-access workspace is missing"))?;
            return Ok(AuthUser {
                user,
                csrf: String::new(),
            });
        }

        Err(AppError::Unauthorized)
    }
}

/// Require a valid CSRF token for an authenticated mutation. The token is tied
/// to the session and only ever exposed to same-origin JavaScript.
pub fn require_csrf(
    state: &AppState,
    headers: &HeaderMap,
    provided: Option<&str>,
) -> Result<(), AppError> {
    let Some(token) = token_from_headers(headers) else {
        return Err(AppError::Unauthorized);
    };
    let Some(session) = state.store.session_by_token(&token) else {
        return Err(AppError::Unauthorized);
    };
    let provided = provided.unwrap_or_default();
    if constant_time_eq(&session.csrf, provided) {
        Ok(())
    } else {
        Err(AppError::Forbidden("missing or invalid CSRF token".into()))
    }
}

pub fn delete_current_session(state: &AppState, headers: &HeaderMap) -> Result<(), StoreError> {
    if let Some(token) = token_from_headers(headers) {
        state.store.delete_session(&token)?;
    }
    Ok(())
}

/// A valid hash used to equalise login timing when the email is unknown.
pub fn dummy_password_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(b"not-a-real-password", &salt)
            .expect("hashing dummy password")
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation() {
        assert_eq!(
            normalize_email(" User@Example.COM ").unwrap(),
            "user@example.com"
        );
        assert!(normalize_email("nope").is_err());
        assert!(normalize_email("a@b").is_err());
        assert!(normalize_email("a b@example.com").is_err());
        assert!(normalize_email("@example.com").is_err());
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("correct horse battery").is_ok());
    }
}
