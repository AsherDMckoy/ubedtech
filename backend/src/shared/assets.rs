//! Static assets: the fingerprinted bundles the frontend pipeline writes
//! into `frontend/dist/` (ADR-12), loaded into memory once at startup and
//! served under their own content-hashed URLs with an immutable cache
//! lifetime. `dist/` is committed, so building or testing the backend
//! never invokes Node; the fingerprint in the filename comes from esbuild,
//! so a changed bundle automatically gets a new URL and the year-long
//! `Cache-Control` can never serve stale chrome.

use std::sync::OnceLock;

use actix_web::{HttpResponse, web};

pub struct Dist {
    css_href: String,
    css: String,
    js_href: String,
    js: String,
    /// Fingerprinted binary assets the bundles reference (the two vendored
    /// font files) — served immutable like everything else in dist.
    fonts: Vec<(String, Vec<u8>)>,
}

/// The dist directory: `APP_FRONTEND_DIST` if set, else `frontend/dist`
/// next to the backend crate (correct for development and tests).
fn dist_dir() -> String {
    std::env::var("APP_FRONTEND_DIST")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../frontend/dist").to_string())
}

/// Loads the two bundles once. Called eagerly from `main.rs` so a
/// deployment without built assets dies at startup with instructions,
/// not mid-request.
pub fn dist() -> &'static Dist {
    static DIST: OnceLock<Dist> = OnceLock::new();
    DIST.get_or_init(|| {
        let dir = dist_dir();
        let mut css = None;
        let mut js = None;
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!(
                "cannot read the frontend dist directory {dir}: {error}. \
                 Run `npm run build` in frontend/ or point APP_FRONTEND_DIST \
                 at the built assets."
            )
        });
        let mut fonts = Vec::new();
        for entry in entries {
            let path = entry.expect("dist directory entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let slot = match name {
                _ if name.ends_with(".woff2") => {
                    let bytes = std::fs::read(&path)
                        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
                    fonts.push((format!("/assets/{name}"), bytes));
                    continue;
                }
                _ if name.starts_with("app-") && name.ends_with(".css") => &mut css,
                _ if name.starts_with("app-") && name.ends_with(".js") => &mut js,
                _ => continue,
            };
            assert!(
                slot.is_none(),
                "more than one app-*{} bundle in {dir}; re-run `npm run build`",
                &name[name.rfind('.').unwrap()..]
            );
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            *slot = Some((format!("/assets/{name}"), content));
        }
        fonts.sort();
        let (css_href, css) =
            css.unwrap_or_else(|| panic!("no app-*.css bundle in {dir}; run `npm run build`"));
        let (js_href, js) =
            js.unwrap_or_else(|| panic!("no app-*.js bundle in {dir}; run `npm run build`"));
        Dist {
            css_href,
            css,
            js_href,
            js,
            fonts,
        }
    })
}

/// `/assets/app-<fingerprint>.css` — referenced from `base.html`.
pub fn css_href() -> &'static str {
    &dist().css_href
}

/// `/assets/app-<fingerprint>.js` — referenced from `base.html`.
pub fn js_href() -> &'static str {
    &dist().js_href
}

/// Badge modifier class for a domain status string, used by templates as
/// `class="badge {{ crate::shared::assets::badge_class(status) }}"`. One
/// map for every page so the same status never gets two colors.
pub fn badge_class(status: &str) -> &'static str {
    match status {
        // "open" is a registration window that is accepting changes now.
        "ready" | "published" | "active" | "enrolled" | "approved" | "open" | "enabled" => {
            "badge-ok"
        }
        // "consumed" is an override that already admitted its enrollment —
        // informational, not a success or failure.
        "pending" | "generating" | "queued" | "upcoming" | "consumed" => "badge-info",
        // A draft grade is amber everywhere: entered, unpublished, invisible
        // to students (instructor-grades reference).
        "amended" | "suspended" | "draft" | "probation" => "badge-warn",
        "rejected" | "failed" | "expired" => "badge-bad",
        // "closed" (a finished window) stays neutral: routine, not an error.
        _ => "",
    }
}

const IMMUTABLE: (&str, &str) = ("Cache-Control", "public, max-age=31536000, immutable");

async fn css() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/css; charset=utf-8")
        .insert_header(IMMUTABLE)
        .body(dist().css.as_str())
}

async fn js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/javascript; charset=utf-8")
        .insert_header(IMMUTABLE)
        .body(dist().js.as_str())
}

async fn font(request: actix_web::HttpRequest) -> HttpResponse {
    let wanted = request.path();
    match dist().fonts.iter().find(|(href, _)| href == wanted) {
        Some((_, bytes)) => HttpResponse::Ok()
            .content_type("font/woff2")
            .insert_header(IMMUTABLE)
            .body(bytes.as_slice()),
        None => HttpResponse::NotFound().finish(),
    }
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.route(css_href(), web::get().to(css));
    cfg.route(js_href(), web::get().to(js));
    for (href, _) in &dist().fonts {
        cfg.route(href, web::get().to(font));
    }
}

/// Structural accessibility audit for a rendered page, shared by every UI
/// flow test (the "automated checks for the critical pages"). String-level
/// on purpose — no HTML-parser dependency — so it catches the regressions
/// that matter (missing labels, captions, landmarks, CSP violations)
/// without pretending to replace the axe pass in frontend/test/.
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use actix_web::http::header;
    use actix_web::{App, test as actix_test};

    #[actix_web::test]
    async fn assets_serve_fingerprinted_with_an_immutable_cache_lifetime() {
        // Wrapped in the security headers like main.rs: the default
        // `Cache-Control: private, no-store` must NOT displace the
        // immutable lifetime the asset handlers set themselves.
        let app = actix_test::init_service(
            App::new()
                .wrap(crate::app::security_headers(
                    crate::config::Environment::Development,
                ))
                .configure(routes),
        )
        .await;

        for (href, content_type, body) in [
            (css_href(), "text/css; charset=utf-8", &dist().css),
            (js_href(), "text/javascript; charset=utf-8", &dist().js),
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

        // The vendored fonts serve the same way: fingerprinted, immutable,
        // correct MIME, byte-identical.
        for (href, bytes) in &dist().fonts {
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
                "font/woff2"
            );
            let served = actix_test::read_body(response).await;
            assert_eq!(&served[..], bytes.as_slice());
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
        // Every document-request status is visually distinct and never
        // color alone (the badge text is the status word itself).
        assert_eq!(badge_class("approved"), "badge-ok");
        assert_eq!(badge_class("generating"), "badge-info");
        assert_eq!(badge_class("rejected"), "badge-bad");
        assert_eq!(badge_class("published"), "badge-ok");
        assert_eq!(badge_class("pending"), "badge-info");
        assert_eq!(badge_class("draft"), "badge-warn");
        assert_eq!(badge_class("amended"), "badge-warn");
        assert_eq!(badge_class("failed"), "badge-bad");
        assert_eq!(badge_class("suspended"), "badge-warn");
        assert_eq!(badge_class("probation"), "badge-warn");
        // Registration-window states (registrar overview).
        assert_eq!(badge_class("open"), "badge-ok");
        assert_eq!(badge_class("upcoming"), "badge-info");
        assert_eq!(badge_class("closed"), "");
        // Override review states.
        assert_eq!(badge_class("active"), "badge-ok");
        assert_eq!(badge_class("consumed"), "badge-info");
        assert_eq!(badge_class("expired"), "badge-bad");
        // Document-type configuration ("disabled" stays neutral — routine).
        assert_eq!(badge_class("enabled"), "badge-ok");
        assert_eq!(badge_class("disabled"), "");
        assert_eq!(badge_class("something-new"), "");
    }

    /// Performance budget (docs/PERFORMANCE.md): the CSS budget catches a
    /// vendored framework or embedded image; the JS budget fits the Alpine
    /// CSP build (~61 KiB minified, ADR-12) and catches a second one.
    #[test]
    fn asset_sizes_stay_inside_the_budget() {
        // 32 → 48 KiB with the ADR-17 polish pass (course dialog/expansion,
        // photo rotation, motion vocabulary) — renegotiated in the open, in
        // the commit that spent it, per FRONTEND.md §7.
        assert!(
            dist().css.len() <= 48 * 1024,
            "app css bundle is {} bytes; budget is 48 KiB uncompressed",
            dist().css.len()
        );
        // Two latin variable font files (ADR-14); each immutable-cached,
        // font-display swap, so they never block first paint.
        assert_eq!(dist().fonts.len(), 2, "expected the two vendored fonts");
        for (href, bytes) in &dist().fonts {
            assert!(
                bytes.len() <= 72 * 1024,
                "{href} is {} bytes; font budget is 72 KiB per file",
                bytes.len()
            );
        }
        assert!(
            dist().js.len() <= 80 * 1024,
            "app js bundle is {} bytes; budget is 80 KiB uncompressed",
            dist().js.len()
        );
    }

    /// No images, no inline styles, no inline handlers, no third-party
    /// URLs anywhere in the templates: critical workflow pages are text,
    /// one stylesheet, one script bundle — all same-origin.
    #[test]
    fn templates_carry_no_images_or_csp_violations() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../frontend/templates");
        let mut checked = 0;
        for sub in ["pages", "components"] {
            let dir = format!("{root}/{sub}");
            if !std::path::Path::new(&dir).exists() {
                continue;
            }
            for entry in std::fs::read_dir(&dir).unwrap() {
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
                if sub == "pages" && name != "base.html" {
                    assert!(
                        source.contains("extends \"pages/base.html\""),
                        "{name}: page does not extend the shared base"
                    );
                }
                checked += 1;
            }
        }
        assert!(
            checked >= 12,
            "expected all page templates, found {checked}"
        );
    }

    /// The token sheet is the contrast source of truth (dist is minified,
    /// so the source file is parsed; CI proves dist matches the sources).
    fn tokens_css() -> &'static str {
        include_str!("../../../frontend/styles/tokens.css")
    }

    fn hex_color(css: &str, token: &str) -> Option<(f64, f64, f64)> {
        let at = css.find(&format!("--{token}: #"))?;
        let hex = &css[at + token.len() + 5..at + token.len() + 11];
        let channel = |i: usize| {
            let value = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        Some((channel(0), channel(2), channel(4)))
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

    /// WCAG AA over the real token sheet, light AND dark: change a token
    /// and this test tells you whether the pair still reads.
    #[test]
    fn design_tokens_meet_wcag_contrast() {
        let sheet = tokens_css();
        let split = sheet
            .find("prefers-color-scheme: dark")
            .expect("tokens.css must define the dark theme");
        let (light, dark) = sheet.split_at(split);
        // The dark block only overrides; anything absent falls back.
        let lookup = |theme: &str, token: &str| {
            let block = if theme == "dark" { dark } else { light };
            hex_color(block, token)
                .or_else(|| hex_color(light, token))
                .unwrap_or_else(|| panic!("token --{token} not found in tokens.css"))
        };

        let text_pairs = [
            ("text-primary", "surface-0"),
            ("text-primary", "surface-1"),
            ("text-primary", "surface-2"),
            ("text-secondary", "surface-0"),
            ("text-secondary", "surface-1"),
            ("text-secondary", "surface-2"),
            ("text-muted", "surface-0"),
            ("text-accent", "surface-0"),
            ("text-accent", "surface-2"),
            ("text-accent", "bg-accent"),
            ("text-success", "bg-success"),
            ("text-warning", "bg-warning"),
            ("text-danger", "bg-danger"),
            ("action-ink", "action"),
            // the gold KPI surface carries brand-ink text (dashboard ref)
            ("text-accent", "action-wash"),
        ];
        for theme in ["light", "dark"] {
            for (fg, bg) in text_pairs {
                let ratio = contrast(lookup(theme, fg), lookup(theme, bg));
                assert!(
                    ratio >= 4.5,
                    "[{theme}] --{fg} on --{bg} is {ratio:.2}:1; WCAG AA text needs 4.5:1"
                );
            }
            // Focus indicator against the page background (non-text: 3:1).
            let ratio = contrast(lookup(theme, "border-accent"), lookup(theme, "surface-0"));
            assert!(
                ratio >= 3.0,
                "[{theme}] --border-accent on --surface-0 is {ratio:.2}:1; needs 3:1"
            );
        }

        // The explicit-choice dark block ([data-theme="dark"]) and the
        // system-preference media block must define IDENTICAL values —
        // otherwise "chose dark" and "OS is dark" silently drift apart and
        // only one of them is contrast-checked.
        let block = |from: usize| {
            let open = sheet[from..].find('{').unwrap() + from + 1;
            let close = sheet[open..].find("\n}").unwrap() + open;
            sheet[open..close]
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with("--"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let chosen = block(
            sheet
                .find("\n:root[data-theme=\"dark\"] {")
                .expect("chosen-dark block"),
        );
        let media_root = split
            + dark
                .find(":root:not([data-theme])")
                .expect("system-dark block");
        let system = block(media_root);
        assert_eq!(
            chosen, system,
            "the [data-theme=\"dark\"] and prefers-color-scheme dark blocks have drifted apart"
        );
    }
}
