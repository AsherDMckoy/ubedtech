# Security

Status: Phase 1 (foundation). Authentication, sessions, CSRF, and license
enforcement do not exist yet — they are Phase 2 and the system is not
deployable until then. This file records what is already enforced and the
baseline every later phase must keep.

## Enforced today (with the test that proves it)

| Control | Where | Proof |
|---|---|---|
| Raw SQL/internal errors never reach clients | `shared/error.rs` maps Database/Template/Internal to a generic body and logs detail server-side | `shared::error::tests::database_errors_never_reach_the_client` |
| Security headers on every response | `app::security_headers` | `app::tests::security_headers_are_present_and_csp_is_alpine_csp_compatible` |
| CSP compatible with Alpine CSP build (no `unsafe-eval`/`unsafe-inline`) | same | same test asserts their absence |
| HSTS in production only | same | `app::tests::hsts_is_production_only` |
| No query strings / headers / cookies / bodies in logs | `shared/observability.rs` logs method, path, status, duration only — redaction by construction | code review; the middleware has no access path that logs them |
| Correlation ids resist log injection | inbound `x-request-id` must be 8–64 chars of `[A-Za-z0-9_-]` or it is replaced | `shared::observability::tests::hostile_inbound_request_id_is_replaced` |
| Config errors never echo values | `config::ConfigError` Display | `config::tests::config_error_display_never_echoes_values` |
| Bounded request bodies | 64 KiB JSON/form, 256 KiB payload defaults in `main.rs` | raise per-route deliberately, never globally |
| No secrets in the repo | `.env` gitignored; `.env.example` contains none; token/secret hashes only in future schema | `.gitignore`; review |
| Fail closed without a license row | startup refuses to serve protected traffic if `institution_license` is empty | `main::load_initial_license` |

## Known-insecure until Phase 2 (do not deploy)

- No login exists; every protected route 401s for everyone.
- `LicenseGate::require_active` is not called on any request path — a
  suspended institution is not actually locked (401 instead of 402).
- CSRF tokens appear in forms but nothing validates them.
- `argon2`/`subtle` are unused dependencies until the password slice.
- The `user_session` schema stores a bare uuid as the session identifier;
  Phase 2.2 replaces it with a stored hash of an opaque high-entropy token.

## Standing rules (CLAUDE.md §2 restated where Phase 1 touches them)

- `.env` is a development convenience only and never overrides real
  environment variables; production secrets come from the deployment's
  secret store.
- There is no development auth bypass in the code. If one is ever added it
  must be behind a cfg that cannot compile into a release binary. The dead
  `DEV_BYPASS_AUTH` env var was removed in Phase 1.
- Every new query takes `institution_id` in the WHERE clause and relies on
  institution-scoped unique constraints; every new sensitive mutation writes
  its audit row inside the same transaction.
- PII redaction in logs is by construction: log call sites may not pass
  user-supplied strings other than the validated correlation id.
