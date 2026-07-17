# Security

Status: through Phase 6 (admin & licensing). Authentication, sessions,
CSRF, license enforcement (hosted + self-hosted signed import), and
institution administration are live and test-backed. This file records
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

## Self-hosted signed licensing

Self-hosted deployments accept license updates only as **platform-signed
files** imported through `POST /license/import` (license-exempt so a locked
deployment can recover; session + admin role required so the audit trail
has a real actor; the Ed25519 signature is the actual authority).

**File format v1 (frozen — ADR-10).** A JSON envelope:

```json
{
  "format": 1,
  "claims_json": "<exact UTF-8 JSON text of the claims>",
  "signature_hex": "<Ed25519 signature over the raw claims_json bytes>"
}
```

`claims_json` parses to: `institution_id`, `deployment_id`,
`license_serial`, `valid_from`, `valid_until` (RFC 3339), `feature_set`.
The signature covers the **byte-exact `claims_json` string**, never a
re-serialization, so verification cannot break on serializer differences.
Verification checks, in order: format version, signature (against
`APP_LICENSE_PUBLIC_KEY`), deployment id (must match the license row's
`deployment_id`), validity window (half-open: `valid_from <= now <
valid_until`), and institution id. Only signature-verified bytes are ever
parsed as claims. Every failure is a fixed-message 422; nothing from the
file is echoed. Proofs: `import::a_signed_license_import_unlocks_a_locked_
deployment`, `import::bad_or_misdirected_license_files_are_rejected`.

**The private signing key never exists on a university deployment.** The
deployment is configured with the PUBLIC key only (`APP_LICENSE_PUBLIC_KEY`,
64 hex chars; unset ⇒ imports refused, the hosted default). The signing key
lives with the platform operator — offline or in an HSM/secrets manager —
and license files are produced platform-side:

1. Generate once, platform-side: an Ed25519 keypair
   (e.g. `openssl genpkey -algorithm ed25519`). Store the private key
   offline; give deployments only the 32-byte public key as hex.
2. To issue: serialize the claims as JSON, sign the exact bytes with the
   private key, emit the envelope above. (The test suite's `import` module
   is the reference implementation of both sides; the binary itself
   contains no signing code — verified by inspection, `SigningKey` appears
   only under `#[cfg(test)]`.)

**Key rotation.** One active key per deployment, rotated by coordinated
reissue:

1. Generate a new keypair platform-side; keep the old private key until
   rotation completes everywhere.
2. For each deployment: issue a fresh license signed with the NEW key,
   deliver it together with the new public key; the operator updates
   `APP_LICENSE_PUBLIC_KEY`, restarts (config is read at startup), then
   imports the new file.
3. Retire (destroy or archive offline) the old private key once all
   deployments run the new public key. A compromised signing key is
   handled the same way, urgently: rotate the public key first — every
   file signed by the stolen key is then rejected — and reissue.

Rotation needs no code change and no downtime beyond the restart; imports
verified with the old key keep working until the public key is swapped,
because verification is against the configured key, not a bundled one.

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
- **Per-role deny tests for enrollment/grades/documents operations** —
  listed as matrix debt in `docs/PERMISSIONS.md`, owed by Phases 4–6/8.
- **MFA, session listing/self-service revocation UI** — not in scope for
  any current phase; revisit after Phase 8.
