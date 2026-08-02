# Architecture Decisions

Numbered record of deviations from
`UB_EDTECH_SYSTEM_DESIGN_AND_ARCHITECTURE.md` /
`UB_EDTECH_IMPLEMENTATION_GUIDE_WITH_CODE.md`, per CLAUDE.md §8. A deviation
without an entry here may not land in a commit.

## ADR-1: SQLx TLS features `runtime-tokio` + `tls-rustls-ring`

- **Original:** the implementation guide's `Cargo.toml` lists a single
  `runtime-tokio-rustls` feature.
- **Replacement:** `runtime-tokio` plus `tls-rustls-ring` (already on disk at
  baseline; kept).
- **Why:** `runtime-tokio-rustls` does not exist as one feature in SQLx
  0.9; runtime and TLS provider are selected separately.
- **Consequences:** none at runtime; dependency resolution simply works.
- **Proof:** the crate compiles and every `#[sqlx::test]` runs (CI gate 3).

## ADR-2: `/health/live` and `/health/ready` replace the single `/health`

- **Original:** guide's `app.rs` had one `/health` returning `"ok"`.
- **Replacement:** `/health/live` (process responsiveness, no dependencies)
  and `/health/ready` (traffic safety, reads a cached flag maintained by a
  background prober that re-checks PostgreSQL every
  `APP_READINESS_INTERVAL_SECS`, default 5s, with the check bounded by the
  same interval).
- **Why:** an orchestrator must distinguish "restart the process" from
  "stop routing traffic"; per-probe database queries would turn probe
  frequency into database load.
- **Consequences:** deployment manifests must use the two new paths; the
  old `/health` path is gone.
- **Proof:** `app::tests::health_live_answers_without_any_state`,
  `app::tests::health_ready_reflects_the_cached_flag`.

## ADR-3: request-id middleware replaces `Logger::default()`

- **Original:** guide's `main.rs` wraps `middleware::Logger::default()`.
- **Replacement:** custom `request_id_middleware`: per-request tracing span
  with correlation id (validated inbound `x-request-id` or generated UUID),
  echoed in the response; completion log carries method, path, status,
  duration only.
- **Why:** `Logger::default()` logs the full request line including query
  strings, which can carry identifiers; and the design requires request
  correlation ids (design doc §21 shared-kernel list).
- **Consequences:** log format changed; clients may supply their own
  correlation ids.
- **Proof:** four tests in `shared::observability::tests`.

## ADR-4: typed `AppConfig` replaces hardcoded runtime values

- **Original:** guide's `main.rs` hardcodes bind address, pool sizes,
  worker id, storage path, and the tracing filter.
- **Replacement:** `config::AppConfig::from_env()` with validation, safe
  defaults, dev-only `.env` (never overriding real env), `.env.example`
  without secrets.
- **Why:** production foundation requirement (Phase 1); hardcoded values
  cannot differ between dev and production.
- **Consequences:** deployments configure via `APP_*` env vars; startup
  aborts on invalid configuration without echoing values.
- **Proof:** eight unit tests in `config::tests`, including one asserting
  error text never echoes configured values.

## ADR-5: HSTS is emitted only in production

- **Original:** design doc §10.4 lists `Strict-Transport-Security` among
  suggested response headers, unconditionally.
- **Replacement:** HSTS is added only when `APP_ENV=production`; all other
  security headers (CSP compatible with the Alpine CSP build,
  `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`,
  `Permissions-Policy`) are unconditional.
- **Why:** development runs plain HTTP on localhost; pinning HSTS there is
  wrong and can poison local browsers for other projects on the same port.
- **Consequences:** production deployments must set `APP_ENV=production`
  (and TLS in front of the binary).
- **Proof:** `app::tests::hsts_is_production_only`.

## ADR-6: document worker takes a shutdown signal

- **Original:** guide's worker loops forever; process exit kills it at an
  arbitrary await point.
- **Replacement:** `DocumentWorker::run` accepts a `watch::Receiver<bool>`;
  main signals it after the HTTP server drains and waits up to
  `APP_SHUTDOWN_TIMEOUT_SECS` for the current job to finish.
- **Why:** a job abandoned mid-render is exactly the orphaned-`running`-row
  defect (CLAUDE.md §1 item 3); graceful shutdown should not manufacture
  orphans. (The reaper for hard crashes is Phase 6.1.)
- **Consequences:** worker shutdown is ordered after server drain; tests
  that spawn the worker must pass a receiver.
- **Proof:** compile-time (signature) + manual SIGTERM verification; a
  crash-window integration test lands with the Phase 6.1 reaper.

## ADR-7: repository documentation lives in `backend/docs/`

See Assumption A1 in `IMPLEMENTATION_PLAN.md`: only `backend/` is a git
repository, and CLAUDE.md §6 requires docs to be committed with code.

## ADR-8: one shared `add_drop_closes_at` deadline for adds and drops

- **Original:** the design docs gave `academic_term` two deadlines —
  `registration_closes_at` checked by adds, `drop_add_closes_at` checked by
  drops — and never resolved whether a student may *add* during the drop/add
  window (CLAUDE.md §1 item 5).
- **Replacement:** migration 0009 consolidates them into a single
  `add_drop_closes_at` column governing both actions (the phase-3 prompt's
  resolved policy). `registration_closes_at` is dropped, not kept as a third
  overlapping knob; the check becomes
  `registration_opens_at < add_drop_closes_at`.
- **Why:** the drop/add window exists so students can swap classes, and a
  swap is a drop plus an add — a policy that allows one but not the other in
  the same window is incoherent and was the unresolved question itself. One
  column also removes an entire class of misconfiguration
  (`registration_closes_at > drop_add_closes_at` was only prevented by a
  CHECK, and the gap between the two produced the undecided behavior).
- **Data migration:** existing rows keep their `drop_add_closes_at` value.
  This preserves drop behavior exactly and extends adds to the end of the
  same window; the alternative (keeping the earlier timestamp) would have
  revoked drop rights students already had.
- **Consequences:** per-term "registration closes early but drops continue"
  policies are no longer expressible. If a real institution needs that, it
  returns as an explicit, tested feature — not as two ambiguously related
  columns. A registrar `deadline` override remains the escape hatch for
  late changes (item 6 implementation).
- **Proof:** `enrollment::tests::one_deadline_governs_both_adds_and_drops`.

## ADR-9: the session row stores its CSRF token, not only the hash

- **Original:** Phase 2 stored only `csrf_secret_hash`; the token existed
  client-side only, returned once by the login JSON response. Sufficient for
  API clients, impossible for server-rendered pages: a GET handler cannot
  put a token it cannot reproduce into a form.
- **Replacement:** migration 0012 adds `user_session.csrf_secret` (the token
  itself); session resolution carries it into `CurrentSession`, and the
  catalog/registration templates embed it in every state-changing form. The
  middleware's constant-time hash comparison is unchanged. Sessions created
  before the column simply no longer resolve (fail closed, re-login).
- **Why:** the no-JavaScript requirement makes server-rendered forms the
  baseline, and every mainstream server-rendered framework (Rails, Django)
  stores the CSRF secret server-side for exactly this reason. A CSRF token
  is not an authenticator: with the session token still stored hash-only, a
  database snapshot yields nothing a cookie-less attacker can replay.
- **Consequences:** an attacker with live database READ access can pair a
  CSRF token with... nothing, absent the session cookie. The hash column is
  now redundant in principle; it stays because the constant-time middleware
  path is tested and this migration is additive (0001 is applied to real
  databases).
- **Proof:** `enrollment::tests::ui::login_catalog_register_and_drop_work_
  as_plain_forms` (token rendered at GET, accepted at POST), existing CSRF
  rejection tests unchanged.

## ADR-10: signed license files carry the signed bytes verbatim (format v1)

- **Original:** the design docs' unwired `signed_license.rs` sketch signed
  `serde_json::to_vec(&claims)` and verified by RE-serializing the parsed
  claims, betting that signer and verifier serialize identically.
- **Replacement:** format v1, frozen before any customer holds a file: a
  JSON envelope `{ format: 1, claims_json, signature_hex }` where
  `claims_json` is the exact UTF-8 JSON text the platform signed and the
  Ed25519 signature covers those raw bytes. The verifier checks the
  signature FIRST and only then parses `claims_json`; a `format` version
  field makes any future change explicit instead of silently ambiguous.
- **Why:** re-serialization equality is fragile — JSON map ordering,
  timestamp precision, and float formatting all vary across serializers
  and versions, and a verification break here bricks a customer's
  deployment. Signing the carried bytes removes the entire failure class
  and is the standard envelope shape (JWS does the same).
- **Consequences:** license files are a few bytes larger (claims appear as
  an embedded string). The claims schema can evolve additively without a
  format bump; renames/removals require `format: 2` and a verifier that
  accepts both during migration. No compatibility impact: `/license/import`
  answered 501 until now, so no v0 files exist.
- **Proof:** `licensing::tests::import::a_signed_license_import_unlocks_a_
  locked_deployment` and `…::bad_or_misdirected_license_files_are_rejected`
  (tampered bytes, foreign key, wrong deployment, expired window, wrong
  institution, unknown format — all rejected with nothing written).

## ADR-11: no frontend framework — PRG pages, one stylesheet, 30 lines of JS

- **Original:** the design docs planned vendored Alpine.js (CSP build) +
  Alpine AJAX for fragment swaps; `base.html` shipped `<script>` tags for
  files that never existed, so every page actually ran unstyled plain HTML
  with console errors.
- **Replacement:** server-rendered pages with POST-redirect-GET as the
  interaction model; one design-system stylesheet and one 30-line
  first-party script (submit-once + `aria-busy`), both embedded in the
  binary and served fingerprinted/immutable. No third-party code at all.
- **Why:** every journey already works and is tested with JavaScript off;
  pages are a few KiB against an immutably-cached stylesheet, so fragment
  swaps have nothing measurable to save; and a vendored framework is a
  standing supply-chain, CSP, and upgrade liability that two `x-target`
  attributes do not repay. The CSP stays at `script-src 'self'` with no
  inline anything.
- **Consequences:** interactions repaint the page (imperceptible at these
  sizes); any future need for in-page updates (live seat counts, a
  dashboard) reopens this ADR with a profile in hand. The `#notifications`
  x-sync region was removed with the dead script tags.
- **Proof:** the UI flow tests exercise every journey without executing a
  script; `templates_carry_no_images_or_csp_violations` pins the
  no-external-code rule; `assets_*` tests pin fingerprinting, caching, and
  compression.

## ADR-12: frontend/ ownership split, repo root, committed dist/ (supersedes ADR-11)

- **Original:** ADR-11 dropped Alpine and kept one embedded stylesheet +
  30-line script inside `backend/web/`. The repository root was `backend/`,
  so `CLAUDE.md`, `FRONTEND.md`, the design docs, and anything outside the
  crate were untracked.
- **Replacement:** the repository root is the project root. `frontend/`
  owns everything a designer touches: Askama templates
  (`frontend/templates/`, pointed at by `backend/askama.toml`), the token
  sheet and component CSS (`frontend/styles/`), first-party JS
  (`frontend/js/`), and the asset pipeline (Node, pinned via lockfile +
  `engines` + `.nvmrc`; esbuild bundles Alpine's CSP build + the CSS and
  content-fingerprints outputs into `frontend/dist/`). `backend/` owns
  serving: it loads the fingerprinted files from `dist/` at startup and
  serves them under `/assets/` with the immutable cache lifetime, plus all
  routing, rendering data, and security middleware.
- **Why:** FRONTEND.md fixes the stack (Alpine CSP build + Alpine AJAX
  enhancement over working HTML forms), and the phase brief requires the
  shared `frontend/` layout; ADR-11's "two x-target attributes don't repay
  a framework" holds only while no in-page interactivity is required —
  the design references (instant local search, dialogs, menus) require it.
  The prohibition ADR-11 protected — nothing works only with JS — remains
  in force via FRONTEND.md §5 and the JS-off flow tests.
- **Consequences:** `frontend/dist/` is committed, so `cargo build`/`cargo
  test` never invoke Node (a fresh checkout builds with Rust alone); CI
  verifies dist matches the sources by rebuilding and diffing. Asset
  fingerprints are computed by esbuild at build time, not by the backend at
  startup; the backend serves whatever fingerprinted names dist contains.
  JS payload grows from ~1 KiB to the Alpine CSP build (~61 KiB min,
  ~19 KiB gzipped, immutably cached) — the performance budget in
  docs/PERFORMANCE.md moves accordingly.
- **Proof:** `assets_*` tests pin fingerprinted serving, immutable caching,
  and compression against the real dist files; the CI dist-diff step proves
  committed assets match the sources; the UI flow tests still never
  execute a script.

## ADR-14: cookie-stamped theming and self-hosted fonts (annotates ADR-11/12)

- **Original:** ADR-11/12's system had a single palette that followed the
  OS via `prefers-color-scheme` only — no user choice, nothing persisted,
  no display typeface, system font stack only. The upgraded reference
  (docs/design-references/dashboard-upgraded.html) demands a selectable
  light/dark/system theme and the Fraunces/Inter pairing, and demonstrates
  both with an inline script + localStorage and a Google Fonts `@import` —
  all three of which the behavioral floor forbids (CSP `script-src 'self'`
  with no inline scripts; no third-party runtime requests).
- **Replacement:** the choice lives in a `ub_theme` cookie
  (light|dark|system, 1 year, SameSite=Lax). The SERVER stamps
  `<html data-theme=…>` at render time — "system" stamps nothing and the
  `prefers-color-scheme` media block in tokens.css decides. The toggle is
  a real form: POST `/ui/theme` (CSRF-checked) sets the cookie and
  redirects back — that is the JS-off path — and the bundled first-party
  script enhances it to flip `data-theme` instantly and persist via the
  same POST. The per-request theme and the toggle's CSRF token reach
  `base.html` through request-scoped task-locals read by free functions
  (the `assets::css_href()` calling pattern), so no template grows new
  fields. Fonts are two vendored latin variable woff2 files, fingerprinted
  by esbuild, served same-origin/immutable, `font-display: swap`.
- **Why cookie-stamped over inline-script:** an inline `<head>` script is
  a CSP exception on every page to fix a flash the server can prevent
  outright — the attribute rides the first byte of HTML, so there is no
  frame rendered in the wrong theme, and the preference works with
  JavaScript disabled, which localStorage never can.
- **Consequences:** the theme is per-browser (cookie), not per-account —
  acceptable for a display preference; the toggle only renders inside a
  session (the CSRF task-local is empty when signed out), so anonymous
  pages follow the cookie/OS without offering the control.
- **Proof:** `theme_is_cookie_stamped_server_side_and_toggles_as_a_plain_
  form` (stamp present/absent, JS-off POST + redirect + cookie, garbage
  refused); `design_tokens_meet_wcag_contrast` covers both dark blocks and
  proves them value-identical; the asset tests pin font serving and the
  per-file font budget.

## ADR-15: catalog search is a server round trip, not the in-page filter

- **Original (FRONTEND.md §4):** catalog search filters instantly in-page
  over the loaded rows, zero round trips; the GET form is the JS-off
  fallback.
- **Replacement:** the catalog search form is a plain GET the server
  answers from the full catalog (`search_catalog`, paginated). The
  in-page instant filter (`data-search-form`) is removed from this screen;
  it remains available for screens that load their entire dataset.
- **Why:** the catalog paginates at 20 rows (demo scale: ~280 sections).
  An in-page filter over one page silently misses everything past it —
  a student searching "MATH" on page 1 concludes no MATH sections exist.
  Truthful-but-slower beats instant-but-wrong; the read path stays one
  request.
- **Consequences:** searching costs a page load; the "Showing N sections"
  line now reflects the server's answer and names the active query.
- **Proof:** the existing `/ui/catalog?q=…` UI tests exercise the
  server-filtered path (`enrollment::tests` catalog cases); with the
  attribute gone, the form submits identically with and without JS.

## ADR-16: collapsible icon rail — the fourth animated surface

- **Original (FRONTEND.md §2):** exactly three animated surfaces (menu,
  dialog, mobile nav sheet). Desktop nav was a static 200 px text rail.
- **Replacement:** the desktop rail is expanded by default (icons +
  labels, 240 px) and collapsible to a 48 px icon rail via a toggle
  button; a collapsed rail also expands over the content on
  `:hover`/`:focus-within` as a preview (the grid column stays 48 px,
  so `main` never reflows). The width/label-fade transition uses
  `--dur-base` and the house enter easing — no new motion values.
- **Why it earns the exception:** this is persistent app-shell
  structural chrome used on every signed-in page — the same category as
  the sanctioned mobile nav sheet, for desktop — not a per-screen
  decoration. Collapsed it returns ~190 px of width to scanning tables.
- **Persistence:** `ub_rail` cookie via `POST /ui/rail`, the exact
  theme-toggle mechanism (ADR-14): server stamps `.rail-collapsed` on
  the shell at render time, no inline script, no flash; JS-off works by
  defaulting to expanded server-side and PRG-reloading on toggle.
- **Consequences:** collapsed labels stay in the DOM and accessibility
  tree at `opacity: 0`; the rail footer (theme, sign out) additionally
  gets `pointer-events: none` collapsed so nothing invisible is
  clickable — tabbing into the rail expands it first. The requested
  Alpine hover controller was skipped: the CSP Alpine build cannot
  evaluate inline `x-data`/`@mouseenter` expressions, and CSS covers
  pointer + keyboard with zero state. `prefers-reduced-motion` (global
  kill, base.css) reduces open/close to the instant state change, like
  every other surface.
- **Proof:** `rail_preference_cookie_round_trips` (POST sets the
  cookie, the next render stamps the class, garbage values 400); the
  axe suite covers every page with the new rail markup; `render-pages`
  compiles the icon macro on every shell.

## ADR-17: expressive-polish amendment — motion vocabulary replaces the surface cap

- **Original (FRONTEND.md §1/§2/§11):** "motion explains, never decorates";
  a hard cap of enumerated animated surfaces; content-load animations,
  count-up numbers, and celebratory motion on the rejected list.
- **Replacement (2026-08-01, demo-readiness):** §2 becomes a closed
  vocabulary of five motion classes — structural chrome, bounded entrance
  choreography (≤ 6 steps, ≤ 400 ms, transform/opacity only, never on
  scanning tables), micro-interactions, post-confirmation state
  celebrations, and count-up-over-server-rendered-value. New token
  `--dur-slow ≈ 350ms` for entrance/celebration only. §1's governing
  principle now names "feel finished / sell the product" as a legitimate
  job on first-impression surfaces.
- **Why:** the product must sell as SaaS at demo, not only function. The
  earned-motion stance produced a correct, austere UI; the owner's
  requirement is beauty. Amending the constitution (rather than per-screen
  exceptions) keeps polish coherent — one vocabulary, tokens only, instead
  of ADR-16-style one-off exceptions accumulating per surface.
- **What did NOT move:** truthful UI (§3 — celebration follows the server's
  answer, never precedes it), the global `prefers-reduced-motion` kill,
  JS-off functionality, WCAG AA, density rules (§6), copy voice, no new
  dependencies, no animation libraries, no raster hero images. Eye candy is
  CSS gradients/shadows/transforms and inline SVG.
- **Consequences:** the tested CSS budget may rise 32 → 48 KiB uncompressed
  in the commit that implements the pass (`asset_sizes_stay_inside_the_
  budget` updated in the same commit — renegotiated in the open). Entrance
  choreography must not delay interactivity: elements are clickable
  mid-entrance, and first meaningful content is server HTML at byte one.
- **Proof:** the existing gates are the proof harness and all stay in
  force — global reduced-motion kill (base.css, manual checklist item 5),
  `assert_page_a11y` on every UI flow, axe over `render-pages`, the asset
  budget test, `templates_carry_no_images_or_csp_violations` (raster art
  stays impossible). Implementation lands in a following session; this ADR
  amends the constitution first so screens follow the doc, not the reverse.

## ADR-18: sign-in photo rotation — ambient imagery on the front door only

- **Original:** the sign-in visual column is a pure-CSS brand gradient;
  FRONTEND.md rejects raster imagery on workflow pages, and the ADR-17
  motion vocabulary has no ambient class.
- **Replacement (2026-08-02):** the visual column crossfades six
  self-hosted, fingerprinted, ~150 KB jpgs (36 s CSS-only cycle) under a
  translucent brand-gradient overlay that keeps the tagline readable.
  Current photos are placeholders awaiting consented campus photography —
  swap the files in `frontend/images/`, rebuild, done.
- **Scope guard:** the sign-in visual column is the ONLY sanctioned home
  for ambient motion and raster imagery — it is a brand surface, not a
  workflow page; the form column renders complete before any photo byte
  arrives (gradient base layer, no layout shift). Reduced motion shows
  the first photo at rest. Workflow pages remain raster-free
  (`templates_carry_no_images_or_csp_violations` still enforces `<img>`
  absence everywhere).
- **Proof:** axe over the rendered login page; the CSP/no-images sweep;
  asset budgets unaffected (photos are separate fingerprinted files, not
  CSS/JS bytes).
- **Retired (2026-08-02, same day):** the placeholder set was removed at
  the owner's request after review; the visual column reverted to the
  original pure-CSS gradient. The scope guard stands: if consented campus
  photography ever lands, THIS surface (and only this surface) is where
  it goes — restore the two-layer crossfade + build-time manifest from
  git history (commit `0d11130` and earlier).
