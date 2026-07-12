use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthenticated,

    #[error("permission denied")]
    Forbidden,

    #[error("resource not found")]
    NotFound,

    #[error("institution license is inactive")]
    InstitutionLocked,

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("template rendering failed")]
    Template(#[from] askama::Error),

    #[error("internal error")]
    Internal,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::InstitutionLocked => StatusCode::PAYMENT_REQUIRED,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Database(_) | Self::Template(_) | Self::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn error_response(&self) -> HttpResponse {
        let code = match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::InstitutionLocked => "institution_locked",
            Self::Validation(_) => "validation_error",
            Self::Conflict(_) => "conflict",
            Self::Database(_) | Self::Template(_) | Self::Internal => "internal_error",
        };

        // Do not expose raw database or internal errors to the client.
        let message = match self {
            Self::Database(_) | Self::Template(_) | Self::Internal => {
                "An internal error occurred".to_owned()
            }
            other => other.to_string(),
        };

        HttpResponse::build(self.status_code()).json(ErrorBody { code, message })
    }
}
