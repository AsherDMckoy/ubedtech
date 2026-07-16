//! CSRF middleware — enforcement, not convention.
//!
//! Every unsafe-method request that carries a resolved session must present
//! the session's CSRF token, either in the `X-CSRF-Token` header or in a
//! `csrf_token` form field (urlencoded bodies are buffered, inspected, and
//! re-injected for the handler). The token is compared in constant time
//! against the hash stored on the session row, so a token minted for one
//! session can never satisfy another.
//!
//! Requests without a session pass through: they hold no ambient authority
//! to forge, and the handler's `Actor` extractor answers 401. Login is the
//! one explicit exemption — it authenticates from the request body alone.

use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::Method;
use actix_web::http::header::CONTENT_TYPE;
use actix_web::middleware::Next;
use actix_web::{HttpMessage, ResponseError, web};
use subtle::ConstantTimeEq;

use crate::identity_access::sessions::{CurrentSession, sha256};
use crate::shared::error::AppError;

pub const CSRF_HEADER: &str = "x-csrf-token";

/// Paths that change state without an existing session's authority.
/// Everything else with a session and an unsafe method needs the token.
/// Both login forms qualify: authentication comes entirely from the body
/// credentials, and pre-auth there is no session-bound token to present.
const EXEMPT_PATHS: &[&str] = &["/api/v1/session/login", "/ui/login"];

pub async fn csrf_middleware(
    mut req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, actix_web::Error> {
    if !unsafe_method(req.method()) || EXEMPT_PATHS.contains(&req.path()) {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    let current = req.extensions().get::<CurrentSession>().cloned();
    let Some(current) = current else {
        // No session, no ambient authority; Actor extraction yields the 401.
        return Ok(next.call(req).await?.map_into_boxed_body());
    };

    let presented = match header_token(&req) {
        Some(token) => Some(token),
        None => form_token(&mut req).await?,
    };

    let Some(presented) = presented else {
        tracing::warn!("state-changing request without a CSRF token");
        return Ok(reject(req));
    };

    // Hash first, then constant-time compare — same shape for every failure.
    let valid: bool = sha256(presented.as_bytes())
        .ct_eq(&current.csrf_token_hash)
        .into();
    if !valid {
        tracing::warn!("state-changing request with an invalid CSRF token");
        return Ok(reject(req));
    }

    Ok(next.call(req).await?.map_into_boxed_body())
}

/// A finished 403, built here (not bubbled as an `Err`) so outer middleware
/// still decorates it like any other response.
fn reject(req: ServiceRequest) -> ServiceResponse<BoxBody> {
    let (req, _) = req.into_parts();
    ServiceResponse::new(req, AppError::Forbidden.error_response())
}

fn unsafe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn header_token(req: &ServiceRequest) -> Option<String> {
    req.headers()
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Pull `csrf_token` out of an urlencoded body, leaving the body readable
/// for the handler. Tokens are lowercase hex, so no percent-decoding is
/// needed — an encoded (hence non-matching) value simply fails closed.
async fn form_token(req: &mut ServiceRequest) -> Result<Option<String>, actix_web::Error> {
    let is_form = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));
    if !is_form {
        return Ok(None);
    }

    let body = req.extract::<web::Bytes>().await?;
    let token = std::str::from_utf8(&body).ok().and_then(|text| {
        text.split('&')
            .find_map(|pair| pair.strip_prefix("csrf_token="))
            .map(str::to_owned)
    });

    // Hand the handler back an untouched copy of what was read.
    let (_, mut payload) = actix_http::h1::Payload::create(true);
    payload.unread_data(body);
    req.set_payload(actix_web::dev::Payload::from(payload));

    Ok(token)
}
