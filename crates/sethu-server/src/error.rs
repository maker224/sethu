//! Unified error type rendered as `{"error": {"code", "message"}}`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("too many requests, slow down and try again later")]
    TooManyRequests,
    #[error("{0}")]
    TooLarge(String),
    #[error("upstream device error: {0}")]
    BadGateway(String),
    #[error("internal server error")]
    Internal,
}

impl AppError {
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "internal server error");
        AppError::Internal
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized => "unauthorized",
            AppError::InvalidCredentials => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::NotFound(_) => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::TooManyRequests => "too_many_requests",
            AppError::TooLarge(_) => "payload_too_large",
            AppError::BadGateway(_) => "bad_gateway",
            AppError::Internal => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            AppError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::BadGateway(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        });
        (self.status(), Json(body)).into_response()
    }
}

impl From<crate::store::StoreError> for AppError {
    fn from(err: crate::store::StoreError) -> Self {
        use crate::store::StoreError;
        match err {
            StoreError::NotFound => AppError::NotFound("not found".into()),
            StoreError::Conflict => AppError::Conflict("already exists".into()),
            StoreError::Corrupt(e) => AppError::internal(format!("storage corrupt: {e}")),
            StoreError::Io(e) => AppError::internal(format!("storage io: {e}")),
        }
    }
}
