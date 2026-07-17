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

    /// Performance budget (docs/PERFORMANCE.md): chrome stays small. The
    /// limits are generous multiples of today's sizes so the test fails on
    /// a regression class (a vendored framework, a base64 image), not on
    /// honest growth.
    #[test]
    fn asset_sizes_stay_inside_the_budget() {
        assert!(
            CSS.len() <= 16 * 1024,
            "app.css is {} bytes; budget is 16 KiB uncompressed",
            CSS.len()
        );
        assert!(
            JS.len() <= 4 * 1024,
            "app.js is {} bytes; budget is 4 KiB uncompressed",
            JS.len()
        );
    }

    /// No images, no inline styles, no inline handlers, no third-party
    /// URLs anywhere in the templates: critical workflow pages are text,
    /// one stylesheet, one small script — all same-origin.
    #[test]
    fn templates_carry_no_images_or_csp_violations() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/web/pages");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(!source.contains("<img"), "{name}: image on a workflow page");
            assert!(!source.contains("<style"), "{name}: inline style block");
            assert!(
                !source.contains(" style=\""),
                "{name}: inline style attribute"
            );
            assert!(!source.contains("onclick="), "{name}: inline event handler");
            assert!(
                !source.contains("http://") && !source.contains("https://"),
                "{name}: external URL — assets must be same-origin"
            );
            if name != "base.html" {
                assert!(
                    source.contains("extends \"pages/base.html\""),
                    "{name}: page does not extend the shared base"
                );
            }
            checked += 1;
        }
        assert!(
            checked >= 12,
            "expected all page templates, found {checked}"
        );
    }

    fn hex_color(css: &str, token: &str) -> (f64, f64, f64) {
        let at = css
            .find(&format!("--{token}: #"))
            .unwrap_or_else(|| panic!("token --{token} not found in app.css"));
        let hex = &css[at + token.len() + 5..at + token.len() + 11];
        let channel = |i: usize| {
            let value = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        (channel(0), channel(2), channel(4))
    }

    fn contrast(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
        let luminance = |(r, g, b): (f64, f64, f64)| 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let (light, dark) = if luminance(a) > luminance(b) {
            (luminance(a), luminance(b))
        } else {
            (luminance(b), luminance(a))
        };
        (light + 0.05) / (dark + 0.05)
    }

    /// WCAG AA, verified against the real stylesheet: change a token and
    /// this test tells you whether the pair still reads.
    #[test]
    fn design_tokens_meet_wcag_contrast() {
        let text_pairs = [
            ("ink", "paper"),
            ("ink", "paper-dim"),
            ("ink-soft", "paper"),
            ("link", "paper"),
            ("action-ink", "action"),
            ("ok-ink", "ok-bg"),
            ("warn-ink", "warn-bg"),
            ("bad-ink", "bad-bg"),
            ("info-ink", "info-bg"),
            ("mute-ink", "mute-bg"),
        ];
        for (fg, bg) in text_pairs {
            let ratio = contrast(hex_color(CSS, fg), hex_color(CSS, bg));
            assert!(
                ratio >= 4.5,
                "--{fg} on --{bg} is {ratio:.2}:1; WCAG AA text needs 4.5:1"
            );
        }
        // Focus indicator against both page backgrounds (non-text: 3:1).
        for bg in ["paper", "paper-dim"] {
            let ratio = contrast(hex_color(CSS, "focus"), hex_color(CSS, bg));
            assert!(ratio >= 3.0, "--focus on --{bg} is {ratio:.2}:1; needs 3:1");
        }
    }
}

/// Structural accessibility audit for a rendered page, shared by every UI
/// flow test (the "automated checks for the critical pages"). String-level
/// on purpose — no HTML-parser dependency — so it catches the regressions
/// that matter (missing labels, captions, landmarks, CSP violations)
/// without pretending to replace a real browser + axe pass, which lives in
/// the manual checklist (docs/FRONTEND_DESIGN_SYSTEM.md).
#[cfg(test)]
pub fn assert_page_a11y(html: &str) {
    assert!(
        html.contains("<html lang="),
        "page must declare its language"
    );
    assert!(
        html.contains(r#"name="viewport""#),
        "page must have a viewport meta tag"
    );
    assert!(html.contains("<title>"), "page must have a title");
    assert!(
        html.contains(r#"class="skip-link""#),
        "page must have the skip link"
    );
    assert!(html.contains("<main"), "page must have a main landmark");

    let h1_count = html.matches("<h1").count();
    assert_eq!(
        h1_count, 1,
        "page must have exactly one h1, found {h1_count}"
    );

    // Every visible form control needs a label (wrapping or for=). Hidden
    // inputs are the only unlabeled controls allowed.
    let visible_controls = html.matches("<input").count()
        - html.matches(r#"type="hidden""#).count()
        + html.matches("<select").count()
        + html.matches("<textarea").count();
    let labels = html.matches("<label").count();
    assert!(
        visible_controls <= labels,
        "{visible_controls} visible form controls but only {labels} labels"
    );

    // Data tables carry captions and column scopes.
    let tables = html.matches("<table").count();
    let captions = html.matches("<caption").count();
    assert_eq!(tables, captions, "{tables} tables but {captions} captions");
    if tables > 0 {
        assert!(
            html.contains(r#"scope="col""#),
            "tables must mark their column headers"
        );
    }

    // Our CSP forbids inline handlers and styles; a page that needs them is
    // a page that silently breaks in production.
    assert!(!html.contains("onclick="), "inline event handler found");
    assert!(!html.contains(" style=\""), "inline style attribute found");
}
