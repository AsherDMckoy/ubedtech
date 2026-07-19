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

    #[error("too many attempts, try again later")]
    RateLimited,

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("conflict: {0}")]
    Conflict(String),

    /// A database invariant the schema is supposed to guarantee was found
    /// broken (e.g. a section without its capacity row). Clients get the
    /// generic 500 — this is never their fault — but unlike `Internal` the
    /// service layer states exactly which invariant failed, and tests can
    /// assert the failure is distinct from an ordinary business conflict.
    #[error("data integrity violation: {0}")]
    Integrity(&'static str),

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
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Integrity(_) | Self::Database(_) | Self::Template(_) | Self::Internal => {
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
            Self::RateLimited => "rate_limited",
            Self::Validation(_) => "validation_error",
            Self::Conflict(_) => "conflict",
            Self::Integrity(_) | Self::Database(_) | Self::Template(_) | Self::Internal => {
                "internal_error"
            }
        };

        // Do not expose raw database or internal errors to the client. The
        // detail goes to the server log instead, where the request-id span
        // ties it back to the failing request.
        let message = match self {
            Self::Integrity(_) | Self::Database(_) | Self::Template(_) | Self::Internal => {
                tracing::error!(error = ?self, "internal error while handling request");
                "An internal error occurred".to_owned()
            }
            other => other.to_string(),
        };

        HttpResponse::build(self.status_code()).json(ErrorBody { code, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::body::MessageBody;

    fn body_text(error: &AppError) -> String {
        let body = error.error_response().into_body();
        let bytes = body.try_into_bytes().expect("in-memory body");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    }

    #[test]
    fn database_errors_never_reach_the_client() {
        // A protocol-level error whose Display text carries internals a client
        // must never see (table names, SQL fragments, connection details).
        let leaky = sqlx::Error::Protocol(
            "SELECT secret_column FROM user_session WHERE token = 'abc'".into(),
        );
        let error = AppError::from(leaky);

        assert_eq!(error.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        let text = body_text(&error);
        assert!(!text.contains("SELECT"), "leaked SQL: {text}");
        assert!(!text.contains("user_session"), "leaked table name: {text}");
        assert!(text.contains("An internal error occurred"));
        assert!(text.contains("internal_error"));
    }

    #[test]
    fn internal_error_body_is_generic() {
        let text = body_text(&AppError::Internal);
        assert!(text.contains("An internal error occurred"));
    }

    #[test]
    fn status_codes_match_error_variants() {
        assert_eq!(
            AppError::Unauthenticated.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(AppError::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(AppError::NotFound.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(
            AppError::InstitutionLocked.status_code(),
            StatusCode::PAYMENT_REQUIRED
        );
        assert_eq!(
            AppError::RateLimited.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            AppError::Validation("x".into()).status_code(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            AppError::Conflict("x".into()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppError::Internal.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn client_facing_variants_keep_their_messages() {
        // Validation/Conflict messages are written by us for users; they must
        // survive so the UI can show a useful reason.
        let text = body_text(&AppError::Conflict("section is full".into()));
        assert!(text.contains("section is full"));
    }
}
