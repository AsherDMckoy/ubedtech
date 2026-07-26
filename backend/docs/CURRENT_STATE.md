# Current State — verified against the code on disk

Date verified: 2026-07-11 (session: Phase 0/1).
Method: read every file in `backend/`, ran the build/test gates, started the
server against the seeded dev database, and probed it with curl. Nothing below
is assumed from the design docs.

Note on repo layout (updated 2026-07-20): the repository root is now the
project root (`ubedtech/`) — moved from `backend/` so `frontend/`,
`CLAUDE.md`, `FRONTEND.md`, and `docs/design-references/` are versioned
(ADR-12). Canonical engineering docs remain at `backend/docs/`. The design
docs at the project root keep their `(1)`/`(2)` download-suffix names.

## What exists and works

- **Build**: `cargo build` succeeds. Rust edition 2024, crate name `backend`.
  SQLx 0.9.0 resolves and compiles with `runtime-tokio` + `tls-rustls-ring` —
  the broken `runtime-tokio-rustls` flag from the implementation guide is
  **already fixed on disk** (CLAUDE.md §1 item 2 — closed, verified by build).
- **Migrations**: six migrations (foundation, academics, enrollment, records,
  documents, licensing) match the guide's schemas and have been applied to the
  dev database `ubedtechdb`. `src/dev/seed.sql` seeds one institution, one dev
  student, one term, and an active hosted license (valid to 2027-07-11).
- **Server starts** (`cargo run`), binds hardcoded `0.0.0.0:8080`, runs
  migrations at startup, refuses to start without an `institution_license`
  row (fails closed), spawns the document worker.
- **`GET /health`** returns 200 `ok`. Single endpoint; no live/ready split;
  does no dependency checking at all.
- **Security headers** are already sent on every response:
  `X-Content-Type-Options`, `Referrer-Policy`, and a CSP that is already
  compatible with the Alpine CSP build (`script-src 'self'`, no
  `unsafe-eval`). Missing: `Strict-Transport-Security`, `Permissions-Policy`,
  and any body-size limits.
- **Error type** `shared/error.rs`: `AppError` with `ResponseError` impl that
  maps Database/Template/Internal to a generic "An internal error occurred"
  JSON body — raw SQL errors do not reach clients. Verified 401 body:
  `{"code":"unauthenticated","message":"authentication required"}`.
- **Enrollment service** implements the guide's registration/drop
  transactions: idempotency double-check, student-term `FOR UPDATE` lock,
  hold/prerequisite/schedule-conflict checks, atomic conditional seat update,
  audit write in the same transaction.
- **Tests**: exactly one test exists —
  `enrollment::tests::only_one_student_gets_the_last_seat` (races two
  registrations for the last seat via `#[sqlx::test]`) — and it **passes**
  against real PostgreSQL. No other test of any kind.
- **Records / documents / licensing services** exist as per the guide:
  grade save/publish with optimistic versioning, schedule query, transcript
  snapshot service, document request/approve/reject, PDF worker with
  `FOR UPDATE SKIP LOCKED` claiming and retry/backoff, filesystem artifact
  store (tmp+rename, content-hash filenames), lock-free `LicenseGate`
  (arc-swap), hosted license status change service, Ed25519 signed-license
  verification helper.

## What fails

- **`cargo fmt --check` FAILS** — formatting diffs in `src/app.rs`.
- **`cargo clippy --all-targets --all-features -- -D warnings` FAILS** — 19
  errors: 18 dead-code warnings (unused `LicenseGate` methods, unused role
  variants, `license_decision`, recovery-route stubs, etc.) and
  `clippy::too_many_arguments` on `AuditWriter::write` (8/7).
- Two of the four CLAUDE.md §6 quality gates are therefore red at baseline.

## What is missing entirely

- **All authentication.** No login/logout routes (`POST /api/v1/session/login`
  → 404). No session table access code, no password hashing code, no
  middleware. `identity_access/` contains only the `Actor` extractor reading
  request extensions that nothing populates. Every protected route returns
  401 for everyone; the system is unusable end to end.
- **License enforcement on requests.** Verified live: with
  `institution_license.status='suspended'`, a protected request returns 401,
  never 402 — `LicenseGate::require_active` is called from no request path.
  `licensing/http.rs` (the license-change fragment handler) is **not
  registered** in `app.rs` at all. Recovery routes are static-string stubs
  (`"status"`, `"import"`).
- **Typed configuration.** `src/config.rs` and `src/db.rs` are empty files.
  `main.rs` reads `DATABASE_URL` directly; bind address, port, pool sizes,
  worker id, document storage path, and the tracing filter are hardcoded
  (filter ignores `RUST_LOG` despite `.env` setting it). No `.env.example`.
- **Request/correlation IDs, redaction, graceful shutdown handling** for the
  worker (spawned with `rt::spawn`, never joined or signaled), CI of any
  kind, `docs/` (before this session), static asset serving (`web/assets/`
  and `web/components/` are empty; `base.html` references
  `/assets/app.css`, `/assets/alpine-ajax.js`, `/assets/alpine-csp.js`
  which do not exist), body-size limits, pagination, `shared/ids.rs`,
  `shared/pagination.rs`.
- **Modules `academics/`, `institution/`, `jobs/`** — empty directories,
  commented out in `main.rs`.
- **`frontend/`** — empty. **`load/`** — empty (no benchmark scripts).

## CLAUDE.md §1 defect list — status verified on disk

1. **Auth/session/CSRF/license middleware not wired — CONFIRMED.** See above.
   `argon2`, `subtle`, and `rand` are dependencies with zero references in
   `src/` (verified by grep).
2. **SQLx feature flag — ALREADY FIXED on disk.** `Cargo.toml` uses
   `runtime-tokio` + `tls-rustls-ring`; lockfile has sqlx 0.9.0; builds.
   (The implementation guide still shows the broken flag; deviation is the
   doc's, not the code's.)
3. **No reaper for orphaned `document_job` rows — CONFIRMED.** The worker
   sets `status='running', locked_at, locked_by` in one transaction, commits,
   then renders outside any transaction. A crash mid-render leaves the job
   `'running'` forever; nothing sweeps by `locked_at`/`locked_by`.
4. **`section_capacity` row not guaranteed — CONFIRMED.** No trigger, no
   transactional creation with `section` (rows are inserted manually only in
   the test fixture and seed). A missing capacity row and a full section both
   fall into the same `reserved.is_none()` path and produce the same
   client-visible conflict error.
5. **Add vs drop deadline policy unresolved — CONFIRMED.** `register_for`
   rejects when `now >= registration_closes_at`; `drop_for` rejects when
   `now >= drop_add_closes_at`. Whether a student may *add* during the
   drop/add window is undecided and untested.
6. **Dead capacity-override branch — CONFIRMED.**
   `enrollment/service.rs:290-309`: when the seat update returns no row, the
   code queries for a `'capacity'` override, but both the override-present
   and override-absent branches return a Conflict error. The query's result
   only changes the error message; no override record (who/why/expiry/
   consumption) is ever honored or written.

## Additional defects found during inspection (not in the §1 list)

7. `.env` contains `DEV_BYPASS_AUTH=1` but **nothing reads it** — dead,
   misleading config. There is no dev auth bypass in code (good), but the
   variable must not survive into Phase 2 where someone might implement it.
   (`.env` is untracked; `.gitignore` covers `.env` and `.env.*`.)
8. Tracing filter is hardcoded in `main.rs`, so `RUST_LOG` in `.env` is
   silently ignored.
9. The document worker's `run()` loop has no shutdown signal; process exit
   mid-render is exactly the crash window of defect 3.
10. `web/` templates exist for pages/fragments but nothing routes or renders
    them (the Askama templates actually used are inline strings in
    `documents/http.rs`); enrollment fragment responses are hardcoded HTML
    strings in `enrollment/http.rs`.
11. `GET /health` runs the full middleware stack but performs no readiness
    checking; there is no way for an orchestrator to distinguish "process up"
    from "can serve traffic".
12. No body-size limits on JSON/form payloads (Actix defaults only).
13. `documents/http.rs` handlers take `web::Data<PgPool>` and query directly
    for rendering (acceptable read adapters, but institution scoping there
    must be covered by tests in Phase 6).

## Behavior snapshot at baseline (verified with curl, 2026-07-11, pre-Phase 1)

| Probe | Result |
|---|---|
| `GET /health` | 200 `ok`, security headers present |
| `POST /api/v1/session/login` | 404 (route does not exist) |
| `POST /api/v1/me/enrollments` (no session) | 401 `unauthenticated` |
| `POST /ui/registration/add` with license `suspended` | 401 (expected 402 once gate is wired) |
| `cargo test` | 1/1 pass (seat-race concurrency test) |
| `cargo fmt --check` | FAIL |
| `cargo clippy -D warnings` | FAIL (19 errors) |

---

## Phase 1 outcome (2026-07-11, same session)

Everything above describes the baseline. Phase 1 changed the following;
per-slice details are in the git history and `ARCHITECTURE_DECISIONS.md`
(ADR-1 through ADR-7):

- All four quality gates are green and enforced by CI
  (`.github/workflows/ci.yml`, PostgreSQL 16 service, plus a release build).
- Typed `AppConfig` (env-validated, dev-only `.env`, `.env.example` with no
  secrets); dead `DEV_BYPASS_AUTH` removed. Bind address, pool bounds,
  timeouts, storage path, worker id all configurable.
- `db::connect_and_migrate` owns the bounded pool (with acquire timeout) and
  the startup migration run.
- `AppError` internal-class responses proven generic by tests; details go to
  the server log inside the request span.
- Request-id correlation middleware with redaction by construction replaced
  `Logger::default()`; `RUST_LOG` is honored.
- `GET /health/live` (no dependencies) and `GET /health/ready` (cached flag,
  background DB prober; no per-probe DB work) replaced `/health`.
- Security headers: CSP unchanged (already Alpine-CSP-compatible), plus
  `X-Frame-Options`, `Permissions-Policy`, and production-only HSTS; global
  body-size limits (64 KiB JSON/form, 256 KiB payload).
- Document worker takes a shutdown signal; main drains HTTP then stops the
  worker within `APP_SHUTDOWN_TIMEOUT_SECS`.
- Test count: 1 → 22.

**Still true (Phase 2+ scope, unchanged):** no authentication of any kind,
license gate not enforced on requests (401-not-402 behavior above), defects
1, 3, 4, 5, 6 from the list open. The §1 defect statuses and the
"missing entirely" list above remain the authoritative gap record except
where this section says otherwise.

---

## Phase 2 outcome (2026-07-14)

**CLAUDE.md §1 item 1 is CLOSED.** The Actor/session/CSRF/license
middleware is wired in `main.rs` and proven over HTTP: a request without a
valid session answers 401
(`identity_access::tests::request_without_a_session_is_401`) and any
request to a locked institution answers 402
(`licensing::tests::locked_institution_answers_402_and_recovery_stays_
reachable`). `argon2`, `subtle`, and `rand` all have calling code now.

Delivered (details in `docs/SECURITY.md`, matrix in `docs/PERMISSIONS.md`,
per-slice commits in git history):

- Argon2id credential storage, configurable documented parameters;
  12-character minimum on every password path.
- Opaque 256-bit session tokens, SHA-256 hash stored (migration 0007 adds
  `token_hash`/`idle_expires_at` to the empty `user_session` table and
  seeds the 7 role codes); idle + absolute expiry; hardened cookie.
- Session middleware populates `Actor`/`CurrentSession` in request
  extensions; extractors 401 when absent; middleware itself never
  allowlists paths.
- Login (`POST /api/v1/session/login`) with uniform failure responses,
  dummy-hash timing equalization, per account+IP fixed-window throttling
  (migration 0008), rotation of any presented session; logout revokes.
- CSRF middleware: session-bound token, header or form field, constant-time
  compare, urlencoded bodies buffered and re-injected; login is the sole
  exemption.
- License middleware outside the session layer: locked ⇒ 402 before
  routing, tested exemption list keeps the recovery surface (health,
  license status/import, locked page, login/logout, `/ui/platform/`)
  reachable; recovery-route stubs replaced with real handlers
  (import remains an honest 501 until Phase 7.1).
- Rotation/revocation triggers: self password change (current password
  required), admin reset, suspension, role grant/revoke — target sessions
  revoked and audited in the same transaction, every trigger HTTP-tested.
- `identity_access/policy.rs` (pure, unit-tested across all 7 roles) +
  role-assignment API; `platform_licensing_admin` unreachable via HTTP.
- `backend bootstrap-platform-admin` one-shot CLI (stdin-only password,
  works while locked/unlicensed, audited) — documented in OPERATIONS.md.
- No dev auth bypass exists (grep + release build verified);
  `cargo build --release` succeeds.
- Test count: 22 → 67 (`cargo test -- --test-threads=4`; unbounded
  parallelism can exhaust local Postgres connections — infra, not code).

**Open defects from the §1 list: 3 (job reaper), 4 (capacity row), 5
(add-vs-drop deadline), 6 (dead override branch)** — owed by Phases 3/4/6
per `docs/IMPLEMENTATION_PLAN.md`. New debt recorded there and in
`docs/PERMISSIONS.md`: per-role deny tests for enrollment/grades/documents,
reverse-proxy IP policy for throttling.

---

## Phase 3 outcome (2026-07-15)

**CLAUDE.md §1 items 4, 5, and 6 are CLOSED** (item 1 closed in Phase 2;
item 2 was already fixed on disk; item 3 — the job reaper — remains open
for Phase 6):

- **Item 4 (capacity row guarantee):** migration 0010 backfills
  `section_capacity` (capacity = active enrollments, exactly full) and a
  trigger creates the row with every section insert on any path; the
  academics service sets the real capacity in the same transaction. A
  missing row now fails as a distinct `Integrity` fault (500, loud in the
  log), never as "section is full". Proofs:
  `missing_capacity_row_fails_distinctly_from_a_full_section`,
  `every_section_gets_a_capacity_row_from_the_trigger`,
  `section_creation_sets_capacity_in_the_same_transaction`.
- **Item 5 (add vs drop deadline):** resolved by the Phase 3 prompt — one
  shared `add_drop_closes_at` governs both actions; migration 0009
  consolidates the two old columns (ADR-8, assumption A11). Proof:
  `one_deadline_governs_both_adds_and_drops`.
- **Item 6 (dead capacity-override branch):** replaced with a real,
  single-use, fully recorded override system: who, rule, required reason,
  expiry, and the consuming enrollment (migration 0011). Capacity override
  raises capacity+enrolled together and reverts on drop. Registrar-only
  grant API. Proofs: `capacity_override_admits_one_student_and_is_
  consumed_once`, `deadline_override_admits_a_late_add_and_a_late_drop`,
  `override_grants_are_registrar_only_validated_and_scoped`.

Also delivered this session (per-slice commits in git history):

- `academics/` module (was an empty directory): terms, courses, sections,
  meetings, prerequisites — registrar/institution_admin commands, audited
  in-tx; current-term query and paginated institution-scoped catalog
  search. `institution/` and `jobs/` remain empty (calendar + instructor
  assignment deferred; not in the Phase 3 prompt).
- Typed registration denials (`enrollment::types::Denial`) replacing
  message-string matching; `EnrollError` converts to `AppError` for JSON.
- Registrar-managed holds (place/release, idempotent, audited) blocking
  registration with their own denial; hold overrides admit single
  registrations.
- Required correctness tests: idempotent resubmission (concurrent +
  sequential), duplicate enrollment, meeting-overlap conflicts,
  prerequisite minimum-grade enforcement, duplicate-drop race, drop-vs-add
  race. The seat-race test stayed green throughout.
- Student-facing pages working with JavaScript off (plain forms, PRG):
  `/ui/login` (new, ADR-9's render-time CSRF token via migration 0012),
  `/ui/catalog`, `/ui/registration` with inline typed feedback for all six
  rejection cases, HTTP-tested end to end. The hardcoded HTML fragment
  stubs in `enrollment/http.rs` are gone.
- Test count: 67 → 92 (`cargo test -- --test-threads=4` locally).

**Open §1 defect: 3 (document_job reaper) — owed by Phase 6.1.** Deferred
from the old plan: institution calendar/events, instructor assignment.

---

## Phase 4 outcome — records & grades (2026-07-15)

(The session prompt numbered this "Phase 4"; it delivers this plan's
"Phase 5 — Records". §1 item 3, the document-job reaper, is now the ONLY
open item from the CLAUDE.md defect list — owed by the documents phase.)

- **Instructor assignments** (`assign_instructor`: registrar/institution_
  admin, target must hold the instructor role, idempotent, audited) and
  **rosters scoped to assignments**: an instructor's roster and grading
  reach exactly their assigned sections — any other real section id answers
  404 (crafted-request tests); a records officer reads any section in the
  institution and nothing beyond it.
- **Grade integrity (migration 0013):** every UPDATE of `grade_record`
  copies the prior row (value, state, author, version) into
  `grade_revision` via trigger; grade rows are undeletable;
  `transcript_snapshot` rows are immutable (UPDATE/DELETE refused).
- **Draft → published → amended workflow:** draft entry can no longer
  rewrite or unpublish a published grade (the pre-existing hole where
  save_draft reset state to 'draft' unconditionally is closed); corrections
  are records-officer commands with a required reason, preserved history,
  and same-transaction audit. Grade-entry window enforced for instructors,
  officer exempt; cross-institution enrollment ids answer 404 instead of
  silently writing nothing.
- **Student visibility:** published/amended-only is enforced in the queries
  (`student_grades`, `academic_history`, snapshot content) — no calling
  convention returns a draft; proven at service level and over the pages.
- **Immutable transcript snapshots:** officer-generated (audited), monotonic
  versions serialized on the student row lock, content proven to exclude
  drafts; students list their own snapshots.
- **Pages (plain forms, no JavaScript):** /ui/instructor, roster page with
  per-row draft entry + pending/draft/published/amended states (entry locks
  once published) + officer publish action, /ui/grades, /ui/history.
- Test count: 92 → 100. Deferred: unofficial transcript print view,
  Cache-Control headers on student views, concurrent snapshot-version race
  test, correction UI (JSON endpoint only).

---

## Phase 5 outcome — documents (2026-07-16)

**CLAUDE.md §1 item 3 is CLOSED — the defect list from the original review
is now fully closed** (items 1, 2, 4, 5, 6 in earlier phases). Delivered
(the session prompt named this "Phase 5"; it completes this plan's
"Phase 6 — Documents"):

- **Reaper:** the worker loop sweeps `running` jobs whose `locked_at`
  exceeds `APP_JOB_STALE_SECS` (default 300) back to `queued` — or to
  terminal `failed` past the attempt budget, failing the request honestly
  — at startup and every 60s. Crash recovery proven exactly as the prompt
  required: commit the 'running' state, run the reaper directly, assert
  'queued', then a live worker completes the request.
- **Worker correctness:** `FOR UPDATE SKIP LOCKED` claiming proven with a
  two-worker race; bounded retries (3) with recorded failure reasons
  proven with a deterministic failing render; completion is idempotent —
  duplicate jobs converge on the single current artifact (partial unique
  index + ON CONFLICT DO NOTHING). PDF rendering moved to the blocking
  pool (`spawn_blocking`); all storage I/O is tokio::fs. Verified by
  inspection: no render or blocking filesystem work on an HTTP runtime
  thread.
- **Workflow:** approval and rejection both require a reason (A20) and are
  officer-only, institution-scoped; approval commits the immutable
  snapshot, the approved state, and the generation job in one
  transaction (denied attempts leave zero state).
- **Storage boundary:** `DocumentStore` trait (write/read), filesystem
  implementation (tmp+rename, hash-sharded), worker generic over it;
  production object-storage adapter documented in OPERATIONS.md.
- **Downloads:** every download passes `downloadable` (owner or officer,
  ready + current, else 404) and re-verifies sha256 against the recorded
  checksum before serving; fixed safe filename; `private, no-store`.
- **Pages:** /ui/documents (request, track, download) and
  /ui/admin/documents (reasoned review queue) as plain forms; full HTTP
  flow test.
- **Bug fixed en route:** enrollment receipts now take `registered_at`
  from `INSERT … RETURNING` — the Rust-side nanosecond timestamp was the
  real cause of the intermittent idempotency-test failure previously
  blamed on pool exhaustion.
- Test count: 100 → 109.

## Phase 6 outcome — institution administration & licensing (2026-07-16)

Delivered by the "Phase 6" session prompt (this plan's Phase 7 plus the
deferred 3.4):

- **Events/holidays** (`institution/` module, migration 0014): admin-only,
  institution-scoped, audited in-tx; `/ui/admin/calendar` plain-form page.
  Calendar data only — policy windows stay governed by term dates (A23).
- **Institution settings**: name + timezone over JSON
  (`/api/v1/institution/settings`), timezone validated against
  `pg_timezone_names`; audited.
- **Document-type configuration** (migration 0015, trigger-guaranteed rows
  like section_capacity): admin toggles per type; `request_for_self`
  checks inside its transaction and fails closed on disabled/missing rows;
  the student form offers only enabled types; in-flight requests are
  untouched (A24).
- **No-bypass proof**: `institution_admin_does_not_bypass_domain_rules` —
  the admin role is refused by enrollment, grades, and documents services
  directly, so no admin route can bypass domain rules.
- **Hosted licensing**: `GET /ui/platform/license` panel (status, change
  history, reasoned suspend/activate, PRG), reachable while locked. The
  acceptance test proves a disabled license answers 402 institution-wide
  on a still-valid session while health/recovery/license-management stay
  reachable, and suspends NO account and revokes NO session. Validity
  window proven half-open at the exact `valid_until` instant.
- **Self-hosted signed licensing**: real `POST /license/import` —
  Ed25519 verification against `APP_LICENSE_PUBLIC_KEY`, format v1 frozen
  (ADR-10: signature over the exact `claims_json` bytes), deployment/
  window/institution checks, update + change record + audit in one
  transaction, gate swap after commit. Requires an authenticated admin
  (A22). The private signing key exists only in the test module — grep
  confirms `SigningKey` appears nowhere in the binary. SECURITY.md now
  documents the format, signing procedure, and key rotation.
- Test count: 109 → 119.

Phase 7 (frontend/UX hardening) not started, per session scope.

## Phase 7 outcome — frontend hardening (2026-07-16)

- **The chrome now actually exists.** base.html had linked /assets/app.css
  and two Alpine scripts that were never created — every page rendered
  unstyled. Now: one design-system stylesheet (~5.5 KiB) and one 30-line
  submit-once script, embedded, fingerprinted, immutable-cached, gzipped,
  license-exempt (ADR-11: no framework, PRG stays the interaction model).
  Nav links now point at routes that exist; dead templates deleted
  (fragments/, unreachable transcript print view that violated our CSP).
- **Duplicate submission closed everywhere:** document requests were the
  last unguarded state-changing form — migration 0016 gives them
  enrollment-style server-minted idempotency keys, raced and proven.
- **Accessibility, automated where honest:** structural audit
  (assert_page_a11y) on all eleven critical pages inside the UI flow
  tests; WCAG contrast computed from the real tokens; budgets (CSS/JS
  size, no images, no external URLs, no inline style/handlers) as tests.
  The audit caught two real defects on first run (caption-less tables).
  What automation can't catch is a documented manual checklist in
  docs/FRONTEND_DESIGN_SYSTEM.md.
- Test count: 119 → 126. Phase 8 (hardening/permissions/performance
  benchmarks) not started, per session scope.

## Phase 8 outcome — final hardening (2026-07-17)

- **Threat review** (SECURITY.md): full §2 baseline table; two real fixes —
  artifact reads confined to the storage root (tampered-row path traversal
  closed) and `Cache-Control: private, no-store` on all dynamic responses
  (Phase 5 debt 5.2 closed); body limits now test-backed (413).
- **Dependency/license CI**: deny.toml + cargo-deny job (advisories,
  yanked, license allowlist built from the actual tree).
- **Benchmarks** (PERFORMANCE.md, load/): A 636k req/s p99 387µs;
  B 3.9k req/s p99 35.6ms (single warm read ~3.3ms); C 106 req/s —
  fsync-bound by the workstation (raw commit 68ms), app delta <5ms.
  First class-C run accidentally proved idempotency under concurrent load.
- **Query plans**: all hot enrollment/document queries on index scans;
  migration 0017 adds the missing section_meeting(section_id) index found
  by inspection.
- **Backup/restore rehearsal** (BACKUP_AND_RESTORE.md): dump, restore,
  boot, verify — all green, with a post-restore integrity check recorded.
- **PERMISSIONS matrix debt: none** — every HTTP operation has a
  test-backed row.
- Test count: 126 → 128. All four critical journeys green end-to-end over
  HTTP (enrollment ui, records ui, documents ui, licensing lock tests).

## Frontend foundation session outcome (2026-07-20)

The `frontend/` structure, asset pipeline, design system, shell, auth
wiring, demo seed, and axe harness — no feature screens (next session).

- **Repo root moved** to the project root (pure-rename commit, history
  follows); CI paths adjusted. ADR-12 records the frontend/backend
  ownership split.
- **Pipeline**: Node isolated in `frontend/` (exact pins, lockfile,
  `.nvmrc`); esbuild bundles `js/app.js` (Alpine CSP build + submit-once/
  busy-label/bfcache/dialog enhancements) and `styles/app.css` (tokens →
  base → components) into content-fingerprinted `dist/` files.
  **dist/ is committed**: backend builds and tests never invoke Node; CI
  rebuilds and diffs to keep it honest.
- **Backend serving**: `shared/assets.rs` loads `frontend/dist/` at
  startup (`APP_FRONTEND_DIST` override, eager check in `main`), serves
  immutable + gzip. Askama renders from `frontend/templates/`.
- **Design system**: `tokens.css` extracted from the reference screens
  (light + dark, both WCAG-tested; two documented AA deviations);
  primitives per FRONTEND.md §9 with exactly three animated surfaces
  (`<details>` menu, native `<dialog>`, `<details>` mobile nav sheet),
  enter-only motion from tokens, global reduced-motion kill switch;
  component macros in `templates/components/ui.html`; every primitive
  rendered once in `pages/gallery.html`; docs/FRONTEND_DESIGN_SYSTEM.md
  rewritten as built.
- **Shell + auth**: base.html is the app shell (desktop rail / mobile
  sheet). Signed-out GET on `/ui/*` 303s to `/ui/login` (API keeps 401);
  CSRF-protected `GET/POST /ui/signout` revokes server-side and clears
  the cookie. Both test-proven.
- **Demo seed**: `src/dev/seed.sql` extended — Fall 2026 (add/drop OPEN),
  six role accounts, full/low-seats/prereq-blocked sections, mixed grade
  roster, advising hold, pending transcript request. Applies twice on a
  fresh database (verified). No waitlist rows: waitlists are not a
  backend feature.
- **a11y harness**: `cargo run -- render-pages` + `frontend/test/a11y.mjs`
  (axe-core in jsdom, color-contrast delegated to the token test) wired
  as `npm test` and into CI; verified to fail on a seeded-violation page.
- **Gates**: fmt/clippy/tests green at every slice; suite now 131.

## Registrar UI session outcome (2026-07-21)

- **Registrar screens** (scanning density, registrar-dashboard reference,
  all over the SAME audited service functions as the JSON API): overview
  at `/ui/registrar` (term tiles, needs-attention worklist, window
  badges, dense sortable/filterable sections table); terms & windows
  management (create + edit the single shared add/drop window —
  `update_term_windows`, new); sections/capacity/meetings/instructor and
  courses/prerequisites management; student lookup with holds and
  academic standing (`set_academic_status`, new; standing is recorded
  designation, holds block — A31); override grants from the student page
  and the full-record review list (`list_overrides`, new).
- **No staff bypass, verified**:
  `ui::staff_pages_commit_on_the_server_and_grant_no_rule_bypass` —
  refused mutations write nothing; successes are committed at redirect
  time. Window edits, holds, and overrides are proven against live
  student registrations in the flow tests.
- **a11y**: eight registrar pages added to `render-pages`/axe (22 pages
  total, all passing). Suite: 131 → 142 tests, green at every slice.

## Institution-admin UI session outcome (2026-07-21)

- **Admin screens** (ui::admin_nav shell, same audited services as the
  JSON API): calendar redesigned onto the design system (existing flow
  test unchanged); settings page for name + IANA timezone (PostgreSQL
  validates the zone; refusals write nothing) and document-type toggles
  with A24 semantics stated inline; account management — lookup by
  username/email, password reset, role grant/revoke, and confirm-gated
  suspension with a required reason.
- **Guardrails proven through the forms**: the reset password really
  rotates the login, self role-changes are refused inline, the platform
  licensing role is ungrantable (403), suspension blocks the next login,
  foreign account ids 404 (`admin_account_pages_work_as_plain_forms`,
  `settings_page_works_as_plain_forms`).
- **a11y**: four pages added to the axe harness (26 total, all passing).
  Suite: 142 → 144 tests, green at every slice.

## Licensing UI + locked-experience session outcome (2026-07-22)

- **License panel on the design system** (`/ui/platform/license`,
  `ui::platform_nav` shell): status card, mode display, change history.
  Hosted mode keeps the status form (reason required; change record +
  audit in one transaction — unchanged services). Self-hosted mode is
  read-only: `set_status` refuses with 422 at the service (A33), the
  panel names signed import as the only path
  (`self_hosted_license_state_is_read_only_in_the_panel`).
- **Access-suspended screen**: while the license is inactive, browser
  GETs on `/ui/` product pages 303 to `/institution-locked`, now a
  design-system page stating that no individual account is disabled and
  linking the reachable surface (license status, sign-in, license
  management). API clients and non-GET requests keep the JSON 402.
  Carve-outs (health, license status/import, session routes, platform
  UI, assets) proven untouched by
  `locked_institution_answers_402_and_recovery_stays_reachable`.
- **A32**: no public document-verification endpoint exists to keep live
  while locked; reminder recorded in the exemption list.
- **a11y**: three pages added to the axe harness — license panel
  (hosted + self-hosted variants) and the locked screen (29 pages
  total, all passing). Suite: 144 → 145 tests, green at every slice.

## Demo-readiness session outcome (2026-07-22, final frontend session)

- **Responsive**: every table on every page now sits in a scroll
  container (the last two — registration, license panel — fixed); no
  horizontal page scroll at 320 px by construction.
- **Accessibility**: axe green over all 29 rendered pages; the manual
  checklist's machine-verifiable items are verified and recorded in
  FRONTEND_DESIGN_SYSTEM.md (focus ring, native dialogs, reduced-motion,
  color independence, print); the NVDA/real-browser items are named as
  the remaining human release gate.
- **Performance**: budgets re-measured and recorded (CSS 18.9 KiB /
  4.3 gz; JS 63.6 KiB / 20.9 gz; both inside budget, enforced by tests);
  fingerprinted immutable assets, no images, two asset requests per
  page, HTML-first — all test-enforced.
- **E2E**: the four demo journeys are continuous seeded HTTP acceptance
  tests (A34 maps journey → test); no Playwright added — the JS-off
  floor is the tested contract.
- **Demo**: `docs/DEMO_SCRIPT.md` — click-paths, seed accounts
  (src/dev/seed.sql, `ub-demo-password`), talking points per step.
- Production asset build reproducible (dist diff clean); release binary
  builds clean. Suite: 145 tests green.

## Demo-dataset session outcome (2026-07-23)

- **`seed-demo` subcommand** (`src/dev/seed_demo.rs`): deterministic
  (fixed RNG seed), idempotent (detects and skips), refuses
  `APP_ENV=production`, layered on seed.sql. Full scale in ~32 s
  (release, synchronous_commit off on its own pool): 900 students, 45
  instructors, 89 courses across five faculties (FEA/FMSS/FST/FHS real,
  Agriculture assumed for Central Farm), 685 sections over four terms,
  5,389 current + 8,562 past enrollments, 8,564 grades, 47 document
  requests across all six states, 73 holds, 18 calendar events, and a
  second institution (STC) for scoping checks. Everything with an
  invariant or audit requirement went through the real services; a
  verification pass (counters, capacity rows, audit coverage) fails the
  command loudly and is asserted again by the dataset acceptance test.
  Suite: 146 tests green (`--test-threads=4` per TESTING.md).
- **Term-shadow fix**: the sectionless DEV-2026 bootstrap term spanned
  the seed-application day and outranked FALL-2026 in `current_term`,
  blanking every student screen — the rehearsed demo could not start.
  DEV-2026 now sits at fixed past dates; FALL-2026 starts no later than
  today; both upsert so re-applying seed.sql repairs existing databases.
- **Over-enrolled section**: not seeded — unrepresentable by design
  (`section_capacity` CHECK `enrolled_count <= capacity`, and the
  service refuses capacity reductions below current enrollment). The
  display-state request lost to the correctness constraint.
- All four demo journeys re-walked against the full dataset over HTTP:
  each scripted step behaves as scripted (catalog states via search —
  see finding 1), registration/drop/hold/grades/documents/license
  lock+carve-outs+restore all verified; seat counts and idempotency
  held throughout.

### UI findings from the demo dataset (recorded, deliberately not fixed)

1. **Catalog pagination vs. the demo script**: 280 current-term sections
   paginate at 20, ordered by course code — the four scripted seed
   states (CMPS-2131/CMPS-3141/MATH-3201/PHYS-2101) no longer share one
   screen. Each is reachable instantly via search; DEMO_SCRIPT.md now
   notes this. A "demo tour" is not worth building; consider pinning or
   a curated default sort only if presenting stays painful.
2. ~~Registrar sections list is unpaginated~~ **Resolved 2026-07-25 as
   an honest cap, not a pager**: the page was already bounded
   (`SECTIONS_SCAN_CAP` = 500, a deliberate one-page scanning design —
   the in-page filter/sort needs the full set loaded), but the cap was
   silent. It now renders a "first 500 shown, narrow with the filter"
   notice when hit, and the 190 KB/282-row render is within the design's
   recorded ceiling. Real pagination only if an actual term exceeds the
   cap.
3. ~~Officer document queue is unpaginated~~ **Resolved 2026-07-25 as an
   honest cap**: the queue was already `LIMIT 100` oldest-first (a
   self-draining worklist — deciding requests is the navigation), but
   truncation was silent. It now states "showing the 100 oldest of N
   pending" when the backlog exceeds the page. Both caps are
   test-pinned.
4. **Catalog count text**: "Showing all 20 sections" renders on every
   page of ~280 matches — "all" is only true when one page exists.
5. **Meeting days render as ISO numbers** in catalog and registration
   tables ("2 13:00–14:15" for Tuesday); the schedule grid uses day
   names. Cryptic across hundreds of rows.
6. **Live worker drains transient document states**: within ~a minute of
   server start the queued/approved jobs render to `ready` and the
   reaper requeues the frozen `generating` rows — exactly as designed.
   All six badges exist at seed time (test-asserted); a screenshot pass
   must capture approved/generating before starting the server's worker
   or show them live via journey 3. There is no worker-off switch.
7. **Academic history has no GPA/credit totals**: with two fully graded
   terms the table is a long wall of rows with no term or cumulative
   summary line.
