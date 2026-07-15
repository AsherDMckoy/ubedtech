# Implementation Plan — Phases 2 through 8

Companion to `CURRENT_STATE.md`. Each phase is a set of vertical slices
(migration + types + policy + service + HTTP adapter + template/fragment +
tests, wired end to end). A slice is done when its acceptance test is green
and committed — never when a handler merely exists. The four quality gates
from CLAUDE.md §6 run after every slice.

Status legend: `[ ]` not started, `[~]` in progress, `[x]` acceptance test
green and committed.

---

## Phase 2 — Identity & access (sessions, CSRF, license gate wiring)

**STATUS: COMPLETE (2026-07-14).** All slices below are green and committed;
CLAUDE.md §1 item 1 is closed (no-session ⇒ 401, locked institution ⇒ 402,
both HTTP-test-proven). See `docs/SECURITY.md` and `docs/PERMISSIONS.md`.

The single highest-priority gap (CLAUDE.md §1 item 1). Nothing else ships
until this is closed. Depends on: Phase 1 (config, errors, health, CI).

### Slices

2.1 `[x]` **Password hashing + credential verification (no HTTP).**
    Argon2id wrapper in `identity_access/password.rs`, parameters from
    config with documented defaults; constant-time verification via
    `subtle`. Unit tests: hash/verify roundtrip, wrong password fails,
    parameters honored, legacy-hash rejection.

2.2 `[x]` **Session creation + store.** Opaque high-entropy token
    (256-bit from `rand`), only SHA-256 hash stored in `user_session`
    (schema migration: rename/add `token_hash` column; the current schema's
    bare `id uuid` as the cookie value is not acceptable). Idle + absolute
    expiry columns from config. Acceptance: integration test proves the raw
    token never appears in the database; expired/revoked sessions do not
    resolve.

2.3 `[x]` **Login/logout HTTP + session middleware.** `POST
    /api/v1/session/login` verifies credentials, rotates session, sets
    `HttpOnly` (+`Secure` in prod, explicit `SameSite=Lax`) cookie;
    middleware resolves the cookie to an `Actor` (roles loaded from
    `user_role`) and inserts it into request extensions; logout revokes.
    Login throttling per account+IP that cannot lock out an account
    permanently (fixed backoff window, not a counter that only ever grows).
    Acceptance: full-lifecycle HTTP test (login → authed request → logout →
    401); request with no/garbage/expired cookie → 401; suspended user → 401
    and existing sessions revoked.

2.4 `[x]` **CSRF middleware.** Session-bound token (hash stored on session
    row, already in schema as `csrf_secret_hash`), required on every
    browser state-changing request (`/ui/*` POST and any cookie-authed
    mutation). Acceptance: HTTP tests for success, missing token, wrong
    token, token from another session.

2.5 `[x]` **License middleware.** Wrap protected scopes with a middleware
    calling `LicenseGate::require_active` before session work; recovery
    routes (`/health/*`, license status/import, locked page) stay outside.
    Register `licensing/http.rs` routes. Replace recovery-route stubs with
    real handlers. Acceptance: HTTP test — suspend license via service →
    protected request returns **402**, recovery routes still 200; reactivate
    → 200. (This test currently cannot pass; see CURRENT_STATE.)

2.6 `[x]` **Session rotation + revocation triggers.** Rotate on login and
    privilege change; revoke on logout, suspension, password reset (bump
    `session_version`). Acceptance tests per trigger. Delivered as:
    self-service password change (current password required, all sessions
    revoked, fresh session to that client), admin password reset, admin
    suspension — each audited in the same transaction.

2.7 `[x]` **Remove dead `DEV_BYPASS_AUTH` from `.env`/docs.** Removed in
    Phase 1; re-verified this session (grep + `cargo build --release`):
    no dev auth bypass exists on any path.

2.8 `[x]` **Role assignment + policy module** (added during Phase 2).
    `identity_access/policy.rs` pure functions (unit-tested across all 7
    roles); institution admins grant/revoke the six institution roles over
    HTTP; the platform role is unreachable in either direction; real
    changes revoke the target's sessions + audit in one transaction;
    retried changes are idempotent and side-effect-free.

2.9 `[x]` **First-platform-admin bootstrap** (added during Phase 2).
    `backend bootstrap-platform-admin` CLI: one-shot (FOR UPDATE guard),
    stdin-only password, MIN_PASSWORD_CHARS enforced, works while
    unlicensed/locked, audited. Documented in OPERATIONS.md.

2.10 `[x]` **Permissions matrix started.** `docs/PERMISSIONS.md` — every
    row present is test-backed; unbacked checks are listed as explicit
    debt for Phases 4–6/8 (feeds slice 8.1).

### Risks
- Session schema change while dev DB already migrated → write additive
  migration 0007, never edit applied migrations.
- Throttling design can itself become a DoS vector — keep per-IP and
  per-account budgets separate and bounded.

---

## Phase 3 — Institution & academics (structure the domain reads)

Depends on: Phase 2 (admin routes need auth).

3.1 `[ ]` **`academics/` module bootstrap + current-term query.**
    Un-comment module; `GET /api/v1/terms/current` + sections listing with
    pagination. Acceptance: institution-scoped listing test (two
    institutions seeded; each sees only its own rows).
3.2 `[ ]` **Term/course/section admin commands** (create/update, registrar
    or institution_admin only) with audit in-transaction. Acceptance:
    role-matrix tests, constraint tests (unique codes per institution).
3.3 `[ ]` **Section creation creates `section_capacity` transactionally**
    (fixes CLAUDE.md §1 item 4 at the source). Also add a backfill
    migration for existing sections and a DB trigger or FK-style guarantee
    so a section without a capacity row cannot exist. Acceptance: registering
    against a section whose capacity row was deleted manually fails loudly
    with a distinct error (test asserts the distinct message/code), and
    section creation cannot commit without the capacity row.
3.4 `[ ]` **Institution calendar/events** (`institution/` module): events
    CRUD + `GET /api/v1/events`, admin-only mutations, audited.
3.5 `[ ]` **Section meetings + instructor assignment** commands with
    room/time validation.

### Risks
- Backfill for 3.3 must handle sections that already have enrollments.

---

## Phase 4 — Enrollment correctness hardening

Depends on: Phase 3 (capacity guarantee), Phase 2 (real actors).
The seat-race test must stay green through every slice here.

4.1 `[ ]` **Resolve add-vs-drop deadline policy (CLAUDE.md §1 item 5).**
    Conservative default: adds are rejected after `registration_closes_at`
    even inside the drop/add window (fail closed; a registrar `deadline`
    override is the escape hatch). Configurable per deployment
    (`ENROLLMENT_ALLOW_ADD_DURING_DROP_ADD`, default `false`). Record as
    ADR + assumption. Acceptance: time-window unit tests for both settings.
4.2 `[ ]` **Implement or delete the capacity override (item 6).**
    Decision: implement for real — registrar-granted `capacity` override
    consumes an override row (who, why, rule, expiry, consuming enrollment
    id column added by migration) and increments capacity+enrolled together
    so the DB `enrolled_count <= capacity` constraint still holds.
    Acceptance: override path test, override-consumed-once test, audit
    contains the override id; dead branch gone.
4.3 `[ ]` **Idempotency + duplicate/replay tests** (same key twice returns
    the first receipt; concurrent same-key requests produce one enrollment).
4.4 `[ ]` **Drop races**: concurrent duplicate drops decrement exactly once;
    drop of the last seat frees exactly one seat for a concurrent add.
4.5 `[ ]` **Registration UI fragments** rendered from Askama templates
    (replace hardcoded HTML strings in `enrollment/http.rs`) fed by one
    registration-page query.

---

## Phase 5 — Records & student views

Depends on: Phase 2; enrollment data from Phase 4 for realistic tests.

5.1 `[ ]` **Grade entry/publish hardening**: instructor-assignment policy
    tests, version-conflict test, publish-only-by-records-officer test,
    grade-entry window (`grade_entry_closes_at`) enforcement (currently
    unchecked — new).
5.2 `[ ]` **Student grades/schedule read models** already exist — add the
    missing tests: students see only published/amended grades and only their
    own; institution scoping; `Cache-Control: private, no-store` headers.
5.3 `[ ]` **Unofficial transcript print view** (`web/pages/
    unofficial_transcript_print.html`) wired to a real route with watermark
    and timestamp.
5.4 `[ ]` **Transcript snapshot invariants**: concurrent snapshot version
    test (two approvals cannot take the same version).

---

## Phase 6 — Documents workflow & worker robustness

Depends on: Phase 5 (snapshots), Phase 2 (document_officer auth).

6.1 `[ ]` **Job reaper (CLAUDE.md §1 item 3).** Periodic sweep in the worker
    loop: `UPDATE document_job SET status='queued', locked_at=NULL,
    locked_by=NULL WHERE status='running' AND locked_at < now() - $stale`
    with attempts respected (goes to `failed` past the cap). Stale threshold
    from config. Acceptance: integration test inserts a fake orphaned
    running job with old `locked_at`, runs one sweep, asserts it is claimed
    and completed by a live worker; terminal-failure path test.
6.2 `[ ]` **Two-workers-one-job test** (`SKIP LOCKED` proof) and
    crash-window test (job claimed, artifact never written → reaper
    requeues, second run completes, exactly one current
    `generated_document` per request — the partial unique index proves it).
6.3 `[ ]` **Artifact download endpoint** with ownership/role authorization,
    `Content-Disposition: attachment`, fixed safe filename, no filesystem
    paths in responses. Acceptance: student can fetch own ready artifact,
    cannot fetch another student's; officer can fetch any in-institution.
6.4 `[ ]` **Document request/approval fragment pages** from `web/` templates
    replacing inline template strings where they belong to pages.
6.5 `[ ]` **Graceful worker shutdown**: worker listens on a shutdown signal
    (watch channel) so in-flight render finishes or the job is released
    before exit.

---

## Phase 7 — Licensing operations

Depends on: Phase 2 (platform admin auth), Phase 6 patterns for tests.

7.1 `[ ]` **Real recovery routes**: license status (JSON + page), license
    import for self-hosted (signed file verification path — `signed_license
    .rs` exists, unwired), locked page. Available while locked.
7.2 `[ ]` **Platform suspend/activate endpoints** (`/api/v1/platform/...`)
    restricted to `PlatformLicensingAdmin`, wired to `LicenseService`,
    snapshot swap after commit. Acceptance: end-to-end 402 flip test
    (extends 2.5), boundary test at exact `valid_until` instant.
7.3 `[ ]` **License panel UI** for the platform operator (`license_panel
    .html`).

### Risk
- On-disk signed license file format is expensive to reverse once customers
  hold files. If Phase 7 reaches import implementation, freeze a canonical,
  versioned serialization first — this is one of the "stop and ask"
  category items per CLAUDE.md §7 if unclear.

---

## Phase 8 — Hardening, permissions matrix, performance

Depends on: everything prior.

8.1 `[ ]` **`docs/PERMISSIONS.md` role × operation matrix** with a test per
    cell (deny-by-default asserted for every role that should not pass).
8.2 `[ ]` **Abuse/input-limit tests**: body-size limits, content-type
    validation, oversized purpose/note fields, malformed UUIDs.
8.3 `[ ]` **Static asset pipeline**: pinned local Alpine CSP build +
    Alpine AJAX in `web/assets/`, hashed filenames, immutable cache
    headers; CSP verified against the real pages.
8.4 `[ ]` **Benchmark suites A/B/C** in `load/` per the design doc's
    benchmark contract, with the required metadata reported; separate
    local-gate, read-path, and transactional numbers.
8.5 `[ ]` **Ops docs**: `OPERATIONS.md`, `BACKUP_AND_RESTORE.md` (incl. a
    tested restore), `PERFORMANCE.md`, `SECURITY.md` finalization.

---

## Cross-phase dependency summary

```
Phase 1 (foundation) ──> Phase 2 (identity/license wiring)
Phase 2 ──> Phase 3 (academics/institution admin)
Phase 3.3 (capacity guarantee) ──> Phase 4 (enrollment hardening)
Phase 2 + 4 ──> Phase 5 (records)
Phase 5 ──> Phase 6 (documents)
Phase 2 ──> Phase 7 (licensing ops)      [6 and 7 can interleave]
All ──> Phase 8 (hardening/benchmarks)
```

## Standing risks

- Every schema change from Phase 2 on must be an **additive migration**;
  0001–0006 are applied to real dev databases.
- The seat-race test is the architecture's proof; it runs in CI on every
  phase that touches enrollment.
- Institution scoping regressions are silent — every new query gets a
  two-institution test.

## Assumptions log

(Phase prompts and CLAUDE.md §7 require assumptions to be recorded here and
flagged in session reports.)

| # | Date | Assumption | Default chosen | Why |
|---|------|------------|----------------|-----|
| A1 | 2026-07-11 | `docs/` must be committable, but only `backend/` is a git repository, so the CLAUDE.md documentation set lives at `backend/docs/`. | `backend/docs/` | Committing docs with code is mandated by CLAUDE.md §6/§10; creating a second repo at the project root would nest the existing one. Revisit if the root becomes a repo. |
| A2 | 2026-07-11 | Production bind address/port are deployment concerns. | Bind configurable via env (`APP_BIND_ADDR`), default `0.0.0.0:8080` preserved. | Matches current observed behavior; no business policy involved. |
| A3 | 2026-07-11 | HSTS must not be emitted in dev (plain HTTP) but must be in prod. | `Strict-Transport-Security` emitted only when `APP_ENV=production`. | Emitting HSTS over plain-HTTP dev would be ignored/wrong; prod requirement comes from design doc §10.4. |
| A4 | 2026-07-14 | "Rotation after privilege change" (CLAUDE.md §2) for a change made by an admin to another user: we cannot rotate a browser we don't hold. | Revoke ALL of the target's sessions (session_version bump, same tx) on role grant, role revoke, password reset, suspension; the target signs in again. | Conservative: no session ever continues under privileges different from those it authenticated into. Roles are also re-read per request, so stale-privilege reads are impossible either way. |
| A5 | 2026-07-14 | May an institution admin edit their own account's privileges? Not specified in the docs. | No: self role changes are 422, self-suspension is 422. Password self-change goes through the normal current-password path instead. | Fail closed: prevents accidental self-lockout of the last admin and removes the self-escalation question entirely. Cheap to relax later. |
| A6 | 2026-07-14 | Who may hold `platform_licensing_admin`? The docs never say how it is granted. | Not grantable or revocable via any HTTP API; minted only by the one-shot `bootstrap-platform-admin` CLI, which refuses once one exists. | The platform operator role guards the license kill-switch; letting institution-side admins touch it would collapse the platform/institution trust boundary. Recovery from a lost credential is a deliberate manual DB operation. |
| A7 | 2026-07-14 | Password policy is unspecified. | Minimum 12 characters (`MIN_PASSWORD_CHARS`), no composition rules, applied to change/reset/bootstrap alike. | Length is the defensible knob (NIST 800-63B); composition rules add support cost without security. Constant is one line to change. |
| A8 | 2026-07-14 | Login throttling client identity: forwarded headers are spoofable and no proxy exists yet. | Key on the socket peer address; per (institution, username, IP) fixed window so the lockout can never be made permanent or global for a victim. | Unspoofable today; documented in SECURITY.md that a reverse-proxy deployment must revisit this deliberately. |
| A9 | 2026-07-14 | Which institution does a login belong to in this single-tenant deployment? | The institution of the license snapshot loaded at startup. | The deployment refuses to start without exactly this license row; no user-supplied institution selector means no cross-institution login probing. |
| A10 | 2026-07-14 | Should login require a CSRF token? | No — login is the single CSRF exemption. | Pre-auth there is no session-bound token to present; the request carries no ambient authority (credentials are in the body, cookie is SameSite=Lax). Logout and everything else require the token. |

| A11 | 2026-07-15 | The Phase 3 prompt resolved item 5 (one shared `add_drop_closes_at` for adds and drops) but not which value existing rows keep when the two old columns consolidate. | Rows keep their `drop_add_closes_at` value; `registration_closes_at` is dropped. | Preserves drop rights exactly as they were; extends adds to the end of the same window (the resolved policy). Keeping the earlier value would have revoked existing drop rights — the anti-conservative direction for students mid-term. ADR-8. |

(Phase 7's license file format will be recorded here when that phase runs.)
