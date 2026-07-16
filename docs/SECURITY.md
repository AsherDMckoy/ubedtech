# Security

Status: Phase 2 (identity & access) implemented. Authentication, sessions,
CSRF, and license enforcement are live and test-backed. This file records
what is enforced, how, and what is deliberately deferred.

## Sessions

- **Opaque server-side tokens.** 256-bit random tokens (OsRng), issued once
  in the `Set-Cookie` header; the database stores only the SHA-256 hash
  (`user_session.token_hash`, unique). A database dump cannot be replayed
  as cookies. Proof: `sessions::raw_token_is_never_stored`.
- **Cookie attributes:** `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` =
  absolute deadline; `Secure` when `APP_ENV=production`. Proof:
  `valid_session_reaches_the_handler_with_the_correct_actor`.
- **Two deadlines**, both explicit: idle (`APP_SESSION_IDLE_SECS`, default
  1800, slides on activity at most once per 60s) and absolute
  (`APP_SESSION_ABSOLUTE_SECS`, default 43200, never slides). Config
  refuses idle > absolute. Proofs: `sessions::idle_expired…`,
  `absolutely_expired…`, `activity_slides_the_idle_deadline`.
- **Resolution fails closed** on: unknown token, revocation, either
  deadline, stale `session_version`, non-active account status, unknown
  role codes in the database (500, never a partial actor).
- **Rotation:** logging in over an existing session revokes it and issues a
  fresh token (`logging_in_again_rotates_the_presented_session`); changing
  your own password revokes every session and re-issues one to that client
  (`password_change_rotates_this_session_and_revokes_all_others`).
- **Revocation triggers:** logout; suspension; admin password reset; any
  role grant/revoke (privilege change ⇒ target's sessions die, same
  transaction as the change and its audit row). `session_version` on
  `user_account` is the kill-everything lever: bumping it invalidates all
  live sessions at resolve time.

## Passwords

- **Argon2id only.** PHC-format hashes with parameters embedded; verify
  rejects any non-argon2id hash (logged as a server fault, client sees the
  generic 401). Parameters from config, documented in `.env.example`:
  `APP_ARGON2_MEMORY_KIB` (default 19456), `APP_ARGON2_TIME_COST` (2),
  `APP_ARGON2_PARALLELISM` (1) — OWASP baseline. Hashing/verification runs
  on the blocking pool, never an Actix worker thread.
- **Minimum length** `MIN_PASSWORD_CHARS = 12` (characters, not bytes),
  enforced on self-service change, admin reset, and bootstrap alike. No
  composition rules (length beats complexity theater).
- **Self-service change requires the current password** — a stolen session
  alone cannot take over the account.
- **Login answers are uniform.** Unknown username, wrong password,
  suspended account, unusable stored hash: byte-identical 401 bodies, and
  Argon2 verification always runs (dummy hash when no credential exists) so
  timing does not distinguish them. Proof:
  `login_failures_all_get_the_same_generic_401`.

## Login throttling

Per (institution, username, client IP) fixed window: `APP_LOGIN_MAX_FAILURES`
(default 10) failures inside `APP_LOGIN_THROTTLE_WINDOW_SECS` (default 900)
⇒ 429 until the window lapses. The window is fixed, not ever-growing, and
scoped per IP, so an attacker cannot permanently lock a victim out globally;
success clears the budget. Proofs: `throttle_locks_an_account_ip_pair_then_
expires`, `successful_login_resets_the_failure_budget`.

**Deployment note:** the client IP is the socket peer address (cannot be
spoofed), never a forwarded header. Behind a reverse proxy every client
collapses to the proxy's address — before deploying one, revisit
`identity_access/http.rs::login` and adopt a trusted-proxy header policy
deliberately.

## CSRF

Real middleware, not advice: every non-safe method with a resolved session
must present the session-bound token — `X-CSRF-Token` header or `csrf_token`
form field — matched in constant time against the hash stored on the session
row. Tokens are per-session; a valid token from another session is rejected.
The **only** exemption is login (both `POST /api/v1/session/login` and the
Phase 3 HTML form `POST /ui/login`), which carries no ambient authority to
forge (authentication comes entirely from the body credentials; the cookie
is `SameSite=Lax`). Proofs: `csrf_missing_wrong_and_cross_session_tokens_
are_403`, `csrf_form_field_is_accepted_and_the_body_survives_for_the_
handler`.

Since Phase 3 the session row also stores the CSRF token itself (not just
its hash) so server-rendered pages can embed it into forms at GET time —
ADR-9. The session cookie token is still stored hash-only, so a database
snapshot remains unreplayable; a CSRF token without its session cookie
grants nothing. Sessions predating the column fail closed (re-login).

## License gate

Middleware outside the session layer: when the deployment's license is not
active, every request answers **402** before any database work — except the
tested exemption list (health probes, license status/import, locked page,
session login/logout, `/ui/platform/` recovery UI), so a platform licensing
admin can sign in and unlock a locked deployment. Proofs: `licensing::
middleware` unit tests both directions, `locked_institution_answers_402_and_
recovery_stays_reachable`, `platform_admin_flips_the_license_end_to_end`.

## Authorization

- Decisions live in services and policy modules (`identity_access/policy.rs`,
  `enrollment/policy.rs`), never in templates or handlers.
  `docs/PERMISSIONS.md` is the test-backed role × operation matrix.
- Institution scoping: admin operations resolve the target inside the
  actor's institution; other institutions' accounts answer 404
  (`admin_powers_stop_at_the_institution_boundary`).
- `platform_licensing_admin` cannot be granted or revoked through any HTTP
  API. The first (only) one is minted by the operator-run
  `bootstrap-platform-admin` subcommand, which refuses once one exists,
  reads the password from stdin only, and audits itself (docs/OPERATIONS.md).
- Admins cannot suspend themselves or edit their own roles.

## Audit

Sensitive identity mutations write their audit row in the SAME transaction
as the change: password change/reset, suspension, role grant/revoke,
license status change, platform-admin bootstrap. Audit detail never
contains passwords, hashes, or token material.

## Logging and secrets

- Auth events log ids only — never the typed username (it may be a
  mistyped password), never token material, never `DATABASE_URL`.
- Request logs carry method/path/status/duration and a validated
  correlation id; no query strings, headers, cookies, bodies, or PII.
- There is **no development auth bypass** in the codebase (verified by
  grep and by `cargo build --release` this session); the only path that
  populates an `Actor` is the session middleware. If a dev convenience is
  ever added it must be `cfg`-excluded from release builds.
- `.env` is dev-only and untracked; `.env.example` contains no secrets.

## Enforced since Phase 1 (still true, still tested)

| Control | Proof |
|---|---|
| Raw SQL/internal errors never reach clients | `shared::error::tests::database_errors_never_reach_the_client` |
| Security headers + Alpine-CSP-compatible CSP (no `unsafe-eval`/`unsafe-inline`) | `app::tests::security_headers_are_present_and_csp_is_alpine_csp_compatible` |
| HSTS in production only | `app::tests::hsts_is_production_only` |
| Correlation ids resist log injection | `shared::observability::tests::hostile_inbound_request_id_is_replaced` |
| Config errors never echo values | `config::tests::config_error_display_never_echoes_values` |
| Bounded request bodies (64 KiB JSON/form, 256 KiB payload) | set in `main.rs`; abuse tests are Phase 8.2 |
| Fail closed without a license row | `main::load_initial_license` refuses startup |

## Deliberately deferred

- **Reverse-proxy IP policy** (see throttling note) — decide when a proxy
  enters the deployment picture.
- **Password reset by email / self-service recovery** — requires the email
  boundary (a real replaceable trait per CLAUDE.md §0); until then reset is
  admin-mediated.
- **Signed license import** (`/license/import` answers an honest 501) —
  Phase 7.1, needs the frozen file format first.
- **Per-role deny tests for enrollment/grades/documents operations** —
  listed as matrix debt in `docs/PERMISSIONS.md`, owed by Phases 4–6/8.
- **MFA, session listing/self-service revocation UI** — not in scope for
  any current phase; revisit after Phase 8.
