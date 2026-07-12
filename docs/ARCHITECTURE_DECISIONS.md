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
