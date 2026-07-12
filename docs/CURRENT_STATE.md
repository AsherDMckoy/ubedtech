# Current State — verified against the code on disk

Date verified: 2026-07-11 (session: Phase 0/1).
Method: read every file in `backend/`, ran the build/test gates, started the
server against the seeded dev database, and probed it with curl. Nothing below
is assumed from the design docs.

Note on repo layout: the project root (`ubedtech/`) is **not** a git
repository; `backend/` is its own git repo (branch `master`, single "Initial
Commit"). `docs/` therefore lives at `backend/docs/` so it can be committed
per CLAUDE.md section 6. `frontend/` and `backend/load/` are empty
directories. The design docs at the project root have `(1)`/`(2)` filename
suffixes (download copies).

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
