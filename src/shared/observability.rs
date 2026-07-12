use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::middleware::Next;
use std::time::Instant;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Install the global JSON subscriber. The filter comes from RUST_LOG when
/// set, with a quiet default otherwise. Call once, before any other work.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,actix_web=info,sqlx=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

/// Per-request correlation middleware.
///
/// Every request runs inside a span carrying a request id (inbound
/// `x-request-id` when well-formed, freshly generated otherwise), and the
/// same id is returned in the response so clients and operators can quote it.
///
/// Redaction policy, enforced here by construction rather than by filtering:
/// only the method, path (never the query string — it can carry tokens or
/// student identifiers), status, and duration are logged. Headers, cookies,
/// and bodies are never logged.
pub async fn request_id_middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, actix_web::Error> {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %req.method(),
        path = %req.path(),
    );

    let started = Instant::now();
    let mut response = next.call(req).instrument(span.clone()).await?;

    let _entered = span.enter();
    tracing::info!(
        status = response.status().as_u16(),
        duration_ms = started.elapsed().as_millis() as u64,
        "request completed"
    );

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }

    Ok(response)
}

/// Inbound correlation ids are echoed into logs and response headers, so they
/// must not be a vehicle for log injection or header abuse: short, printable,
/// unambiguous characters only.
fn is_valid_request_id(value: &str) -> bool {
    (8..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test as actix_test;
    use actix_web::{App, HttpResponse, middleware, web};

    #[test]
    fn request_id_validation_accepts_reasonable_ids() {
        assert!(is_valid_request_id("abc-123_XYZ"));
        assert!(is_valid_request_id(&Uuid::new_v4().to_string()));
    }

    #[test]
    fn request_id_validation_rejects_hostile_ids() {
        assert!(!is_valid_request_id("short"));
        assert!(!is_valid_request_id(&"a".repeat(65)));
        assert!(!is_valid_request_id("evil\ninjected=line"));
        assert!(!is_valid_request_id("spaces are bad"));
        assert!(!is_valid_request_id(""));
    }

    async fn probe() -> HttpResponse {
        HttpResponse::Ok().body("ok")
    }

    #[actix_web::test]
    async fn response_carries_a_generated_request_id() {
        let app = actix_test::init_service(
            App::new()
                .wrap(middleware::from_fn(request_id_middleware))
                .route("/probe", web::get().to(probe)),
        )
        .await;

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/probe").to_request(),
        )
        .await;

        let id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("x-request-id header present")
            .to_str()
            .unwrap();
        assert!(is_valid_request_id(id), "generated id is well-formed: {id}");
    }

    #[actix_web::test]
    async fn valid_inbound_request_id_is_echoed() {
        let app = actix_test::init_service(
            App::new()
                .wrap(middleware::from_fn(request_id_middleware))
                .route("/probe", web::get().to(probe)),
        )
        .await;

        let request = actix_test::TestRequest::get()
            .uri("/probe")
            .insert_header((REQUEST_ID_HEADER, "client-supplied-id-42"))
            .to_request();
        let response = actix_test::call_service(&app, request).await;

        assert_eq!(
            response.headers().get(REQUEST_ID_HEADER).unwrap(),
            "client-supplied-id-42"
        );
    }

    #[actix_web::test]
    async fn hostile_inbound_request_id_is_replaced() {
        let app = actix_test::init_service(
            App::new()
                .wrap(middleware::from_fn(request_id_middleware))
                .route("/probe", web::get().to(probe)),
        )
        .await;

        let request = actix_test::TestRequest::get()
            .uri("/probe")
            .insert_header((REQUEST_ID_HEADER, "bad id with spaces"))
            .to_request();
        let response = actix_test::call_service(&app, request).await;

        let id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_ne!(id, "bad id with spaces");
        assert!(is_valid_request_id(id));
    }
}
