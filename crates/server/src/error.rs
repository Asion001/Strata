use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not signed in")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("this folder is read-only")]
    ReadOnly,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("range not satisfiable")]
    RangeNotSatisfiable,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::BadRequest(msg.into())
}

pub fn conflict(msg: impl Into<String>) -> ApiError {
    ApiError::Conflict(msg.into())
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => ApiError::NotFound,
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
                ApiError::Conflict("already exists".into())
            }
            e => ApiError::Internal(e.into()),
        }
    }
}

impl From<opendal::Error> for ApiError {
    fn from(e: opendal::Error) -> Self {
        if strata_storage::readonly::is_read_only_refusal(&e) {
            return ApiError::ReadOnly;
        }
        match e.kind() {
            opendal::ErrorKind::NotFound => ApiError::NotFound,
            opendal::ErrorKind::AlreadyExists => ApiError::Conflict("already exists".into()),
            opendal::ErrorKind::PermissionDenied => ApiError::Forbidden,
            _ => ApiError::Internal(e.into()),
        }
    }
}

impl From<strata_storage::paths::PathError> for ApiError {
    fn from(e: strata_storage::paths::PathError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden | ApiError::ReadOnly => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let msg = match &self {
            ApiError::Internal(e) => format!("internal error: {e}"),
            other => other.to_string(),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}
