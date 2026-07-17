//! Static assets: one stylesheet, one small script, embedded in the binary
//! and served under content-fingerprinted URLs with an immutable cache
//! lifetime. No filesystem reads at request time, no build step — the
//! fingerprint is derived from the embedded bytes at startup, so a changed
//! file automatically gets a new URL and a year-long `Cache-Control` can
//! never serve stale chrome.

use std::sync::OnceLock;

use actix_web::{HttpResponse, web};
use sha2::{Digest, Sha256};

pub const CSS: &str = include_str!("../../web/assets/app.css");
pub const JS: &str = include_str!("../../web/assets/app.js");

fn fingerprint(content: &str) -> String {
    hex::encode(&Sha256::digest(content.as_bytes())[..8])
}

/// `/assets/app-<fingerprint>.css` — referenced from `base.html`.
pub fn css_href() -> &'static str {
    static HREF: OnceLock<String> = OnceLock::new();
    HREF.get_or_init(|| format!("/assets/app-{}.css", fingerprint(CSS)))
}

/// `/assets/app-<fingerprint>.js` — referenced from `base.html`.
pub fn js_href() -> &'static str {
    static HREF: OnceLock<String> = OnceLock::new();
    HREF.get_or_init(|| format!("/assets/app-{}.js", fingerprint(JS)))
}

/// Badge modifier class for a domain status string, used by templates as
/// `class="badge {{ crate::shared::assets::badge_class(status) }}"`. One
/// map for every page so the same status never gets two colors.
pub fn badge_class(status: &str) -> &'static str {
    match status {
        "ready" | "published" | "active" | "enrolled" | "approved" => "badge-ok",
        "pending" | "generating" | "queued" | "draft" => "badge-info",
        "amended" | "suspended" => "badge-warn",
        "rejected" | "failed" | "expired" => "badge-bad",
        _ => "",
    }
}

const IMMUTABLE: (&str, &str) = ("Cache-Control", "public, max-age=31536000, immutable");

async fn css() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/css; charset=utf-8")
        .insert_header(IMMUTABLE)
        .body(CSS)
}

async fn js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/javascript; charset=utf-8")
        .insert_header(IMMUTABLE)
        .body(JS)
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.route(css_href(), web::get().to(css));
    cfg.route(js_href(), web::get().to(js));
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use actix_web::http::header;
    use actix_web::{App, test as actix_test};

    #[actix_web::test]
    async fn assets_serve_fingerprinted_with_an_immutable_cache_lifetime() {
        let app = actix_test::init_service(App::new().configure(routes)).await;

        for (href, content_type, body) in [
            (css_href(), "text/css; charset=utf-8", CSS),
            (js_href(), "text/javascript; charset=utf-8", JS),
        ] {
            assert!(
                href.starts_with("/assets/app-") && href.len() > "/assets/app-.css".len(),
                "fingerprint missing from {href}"
            );
            let response = actix_test::call_service(
                &app,
                actix_test::TestRequest::get().uri(href).to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{href}");
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "public, max-age=31536000, immutable"
            );
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                content_type
            );
            let served = actix_test::read_body(response).await;
            assert_eq!(served, body.as_bytes());
        }
    }

    #[actix_web::test]
    async fn assets_compress_when_the_client_accepts_gzip() {
        // Same middleware stack position as main.rs.
        let app = actix_test::init_service(
            App::new()
                .wrap(actix_web::middleware::Compress::default())
                .configure(routes),
        )
        .await;

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri(css_href())
                .insert_header((header::ACCEPT_ENCODING, "gzip"))
                .to_request(),
        )
        .await;
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_ENCODING)
                .map(|value| value.to_str().unwrap()),
            Some("gzip"),
            "stylesheet must be served compressed"
        );
    }

    #[test]
    fn every_known_status_has_a_stable_badge_color() {
        // The same status must never render in two colors on two pages.
        assert_eq!(badge_class("ready"), "badge-ok");
        assert_eq!(badge_class("published"), "badge-ok");
        assert_eq!(badge_class("pending"), "badge-info");
        assert_eq!(badge_class("amended"), "badge-warn");
        assert_eq!(badge_class("failed"), "badge-bad");
        assert_eq!(badge_class("suspended"), "badge-warn");
        assert_eq!(badge_class("something-new"), "");
    }
}
