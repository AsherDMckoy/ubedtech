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

## Phase 3 — Academics structure + enrollment correctness + student UI

**STATUS: COMPLETE (2026-07-15)** except 3.4 (institution calendar) and
instructor assignment from 3.5, which the Phase 3 session prompt did not
include — they move to the next phase that needs them. The Phase 3 prompt
absorbed the old "Phase 4 — enrollment hardening" slices (4.1–4.5 below);
its "Phase 4" is records (this plan's Phase 5). CLAUDE.md §1 items 4, 5,
and 6 are closed.

3.1 `[x]` **`academics/` module + current-term query + catalog** —
    paginated, institution-scoped (`catalog_and_current_term_are_
    institution_scoped`).
3.2 `[x]` **Term/course/section/meeting/prerequisite commands** (registrar
    or institution_admin, audited in-tx; unique-code conflicts are 409s)
    — `academics_commands_enforce_the_role_matrix`,
    `codes_are_unique_per_institution_not_globally`.
3.3 `[x]` **Capacity-row guarantee (item 4)** — migration 0010 backfill
    (capacity = active enrollments) + trigger; service creates real
    capacity in the same tx; missing row fails as `Integrity`, distinct
    from "section is full" — `missing_capacity_row_fails_distinctly_from_
    a_full_section`, `every_section_gets_a_capacity_row_from_the_trigger`.
3.4 `[x]` **Institution calendar/events** — delivered with Phase 7 (the
    "Phase 6" session prompt); see 3.4 under Phase 7 below.
3.5 `[x]` **Meetings done** (day/time/room validation); instructor
    assignment delivered in the records phase
    (`instructor_assignment_is_validated_scoped_and_idempotent`).
4.1 `[x]` **Deadline policy resolved (item 5)** — the phase prompt resolved
    it differently from this plan's earlier sketch: ONE shared
    `add_drop_closes_at` column governs adds and drops (migration 0009
    consolidates, no third column, no config flag). ADR-8, assumption A11.
    Proof: `one_deadline_governs_both_adds_and_drops`.
4.2 `[x]` **Capacity override implemented for real (item 6)** — dead branch
    gone; single-use override raises capacity+enrolled together, is stamped
    with the consuming enrollment, and the bump reverts when that
    enrollment drops. `capacity_override_admits_one_student_and_is_
    consumed_once`, `override_grants_are_registrar_only_validated_and_
    scoped`, `deadline_override_admits_a_late_add_and_a_late_drop`.
4.3 `[x]` **Idempotency proofs** — `idempotent_resubmission_returns_the_
    original_receipt` (concurrent + sequential same key ⇒ one enrollment,
    original receipt).
4.4 `[x]` **Drop races** — `concurrent_duplicate_drops_release_exactly_one_
    seat`, `a_drop_racing_a_registration_keeps_the_counter_honest`.
4.5 `[x]` **Student pages from Askama templates** (hardcoded fragments
    deleted): login page, catalog search/browse, registration panel;
    register/drop as plain form posts with PRG on success and inline typed
    denial feedback (409) for full/prerequisite/duplicate/conflict/hold/
    closed-window. Holds: registrar place/release
    (`holds_block_registration_until_released_or_overridden`). UI proofs:
    `ui::login_catalog_register_and_drop_work_as_plain_forms`,
    `ui::every_rejection_case_renders_inline_feedback`.

---

## Phase 5 — Records & student views

**STATUS: COMPLETE (2026-07-15)** — delivered by the session whose prompt
named it "Phase 4"; remaining debt listed under 5.2/5.3/5.4. Added beyond
the plan: grade REVISION HISTORY by database trigger (migration 0013, no
service path can skip it), a records-office CORRECTION workflow (state
`amended`, required reason, prior value + author preserved), instructor
assignment + rosters scoped to assignments, officer-generated immutable
transcript snapshots, academic-history views, and the instructor/officer/
student pages.

5.1 `[x]` **Grade entry/publish hardening** — assignment scoping with
    crafted-request 404s (`rosters_are_visible_only_for_assigned_sections`,
    `unassigned_instructors_cannot_grade_and_students_never_see_drafts`),
    version conflicts + officer-only publish + published-grades-immune-to-
    draft-entry (`corrections_preserve_prior_value_and_author_in_history`),
    grade-entry window enforced for instructors with the officer exempt
    (`grade_entry_window_binds_instructors_not_the_officer`).
5.2 `[~]` **Student read models** — published/amended-only proven at the
    query level and over HTTP pages; institution scoping in every query.
    The `Cache-Control: private, no-store` debt was closed in Phase 8
    (default header on every dynamic response, test-backed).
5.3 `[ ]` **Unofficial transcript print view** — deferred; `/ui/history` is
    the student academic-history view, the watermarked print view remains.
5.4 `[~]` **Snapshot invariants** — immutability now database-enforced
    (trigger, tested) and versions serialize on the student row lock; the
    dedicated concurrent-version race test remains.

---

## Phase 6 — Documents workflow & worker robustness

**STATUS: COMPLETE (2026-07-16)** — delivered by the session whose prompt
named it "Phase 5". CLAUDE.md §1 item 3 is closed; **every item on the §1
defect list is now closed.** Also delivered beyond the plan: approval now
requires a reason like rejection (A20), the render runs on the blocking
pool, completion is idempotent (duplicate jobs converge on the one
artifact), and downloads re-verify the sha256 checksum on every read.

6.1 `[x]` **Job reaper (item 3)** — sweep in the worker loop (startup +
    every 60s), threshold `APP_JOB_STALE_SECS`; requeue or terminal-fail
    past the attempt budget, request failed honestly. Proofs:
    `a_crashed_workers_job_is_reaped_and_completed_by_a_live_worker`
    (commit 'running', reap directly, back to 'queued', live worker
    completes), `reaping_past_the_attempt_budget_fails_terminally`,
    `the_reaper_leaves_live_jobs_alone`.
6.2 `[x]` **Concurrency proofs** — `two_workers_cannot_claim_the_same_job`
    (SKIP LOCKED), `duplicate_jobs_never_produce_a_second_artifact`
    (partial unique index + ON CONFLICT DO NOTHING),
    `failed_renders_retry_with_recorded_reasons_then_stop`.
6.3 `[x]` **Authorized downloads** — owner or officer, institution-scoped,
    ready+current only, checksum re-verified, fixed safe filename, no
    paths in responses (`downloads_are_authorized_and_checksum_verified`).
6.4 `[x]` **Pages** — /ui/documents (request/track/download) and
    /ui/admin/documents (reasoned approve/reject queue), plain forms, full
    HTTP flow test.
6.5 `[x]` **Graceful worker shutdown** — delivered in Phase 1 (watch
    channel); the reaper now also covers hard kills.

---

## Phase 7 — Licensing operations & institution administration

**STATUS: COMPLETE (2026-07-16)** — delivered by the session whose prompt
named it "Phase 6". Also delivered beyond this plan: institution settings
(name/timezone), per-institution document-type configuration (fail-closed
enforcement inside the request transaction), plan item 3.4 (events/
holidays + admin calendar page), and the explicit admin-no-bypass proof.

7.1 `[x]` **Real recovery routes** — `/license/status` (Phase 2),
    `/institution-locked` (Phase 2), and now a REAL `POST /license/import`:
    Ed25519-signed file verified against `APP_LICENSE_PUBLIC_KEY`
    (format v1 frozen, ADR-10 — signature covers the exact `claims_json`
    bytes); deployment id, validity window, and institution checked; row
    update + `license_change` + audit in ONE transaction; gate swap after
    commit. Reachable while locked, requires an authenticated admin (A22).
    Proofs: `import::a_signed_license_import_unlocks_a_locked_deployment`,
    `import::bad_or_misdirected_license_files_are_rejected`.
7.2 `[x]` **Platform suspend/activate** — existed via
    `/ui/platform/institutions/{id}/license` since Phase 2 (no separate
    `/api/v1/platform/` surface needed; ponytail: one route is enough).
    New: the end-to-end lock test also proves NO account is suspended and
    NO session revoked as a side effect
    (`a_disabled_license_locks_the_institution_but_suspends_nobody`), and
    the half-open validity boundary is unit-tested
    (`the_validity_window_is_half_open`).
7.3 `[x]` **License panel UI** — `GET /ui/platform/license`
    (`license_panel.html`): current license, change history, reasoned
    suspend/activate form (PRG); license-exempt so it works while locked.
3.4 `[x]` **Institution calendar/events** (deferred from Phase 3) —
    `institution/` module, migration 0014, `/ui/admin/calendar`;
    admin-only, scoped, audited, holidays are calendar data only (A23).

### Risk (resolved)
- The signed license file format was the "stop and ask" candidate; it was
  frozen as versioned format v1 BEFORE any file exists (import previously
  answered 501, so there is nothing to migrate) — ADR-10 records the
  envelope and its evolution rules. No ask needed: no irreversible state
  existed.

---

## Phase 7.5 — Frontend hardening (the session prompt's "Phase 7")

**STATUS: COMPLETE (2026-07-16).** The design system now actually exists:
`base.html` had linked assets that were never created. Delivered: design
system stylesheet + fingerprinted/immutable/gzipped asset serving +
30-line submit-once script (ADR-11: no framework, PRG stays), nav fixed,
dead templates deleted, document-request idempotency keys (migration
0016 — the last unguarded form), structural a11y audit on all eleven
critical pages inside the UI flow tests, WCAG contrast + size-budget +
no-image/no-external-URL tests, `docs/FRONTEND_DESIGN_SYSTEM.md` with the
manual accessibility checklist, `docs/PERFORMANCE.md` frontend budgets.
This supersedes 8.3 below (done without Alpine, per ADR-11).

## Phase 8 — Hardening, permissions matrix, performance

Depends on: everything prior.

8.1 `[x]` **`docs/PERMISSIONS.md` role × operation matrix** — completed
    across Phases 2–6; Phase 8 closed the debt section: every HTTP
    operation has a matrix row with proving tests, none remain untested.
8.2 `[~]` **Abuse/input-limit tests** — body-size limit now test-backed
    (`oversized_request_bodies_are_refused`, 413); oversized text fields
    covered by per-field validation tests since their phases; malformed
    UUIDs answer 400 from the extractors (untested convention — remaining
    debt, low risk).
8.3 `[x]` **Static asset pipeline** — delivered in Phase 7.5, without
    Alpine (ADR-11): hashed filenames, immutable cache headers, budgets
    and CSP compliance verified against the real pages by tests.
8.4 `[x]` **Benchmark suites A/B/C** in `load/` — run 2026-07-17 with
    full metadata in PERFORMANCE.md (in-process 636k req/s; read path
    3.9k req/s; durable writes 106 req/s, fsync-bound by the workstation).
    Query plans inspected; migration 0017 fixed the one real find
    (unindexed section_meeting.section_id).
8.5 `[x]` **Ops docs** — BACKUP_AND_RESTORE.md with a performed dump +
    restore + boot rehearsal; PERFORMANCE.md benchmarks; SECURITY.md
    threat review (two fixes landed); dependency/license policy in CI
    (deny.toml).

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
| A12 | 2026-07-15 | Are overrides reusable until expiry, or one-shot? The prompt requires recording "which enrollment transaction consumed it", which only makes sense once. | Every override type is single-use: claimed under row lock, stamped with the consuming enrollment in the same tx. The capacity override's extra seat is the student's, not the section's — it reverts when that enrollment drops. A student needing N exceptional adds needs N overrides (or the hold released / capacity raised properly). | Fail-closed academic integrity: a lingering reusable override is a hidden boolean with an expiry. The registrar's real remedies (release the hold, raise capacity) stay the honest path. |
| A13 | 2026-07-15 | Which roles grant overrides and manage holds? Docs are silent. | Registrar only (institution_admin excluded). Academic structure (terms/courses/sections/meetings/prereqs) is registrar OR institution_admin. | Overrides/holds bypass integrity rules — one accountable authority. Structure setup must work before a registrar account exists, so the admin may also do it. Policy functions are one line to widen. |
| A14 | 2026-07-15 | The prompt's no-JavaScript requirement makes browser flows the baseline, but no HTML login existed (Phase 2 login is JSON). | Added `GET/POST /ui/login` (small, shares the JSON login's code path); it joins login as the CSRF exemption (extends A10) and the license-exempt list. | Without it the student pages are unreachable in a real browser with JS off; the alternative (requiring a JSON client to bootstrap a cookie) defeats the requirement. |
| A15 | 2026-07-15 | Does a `deadline` override let a registrar add a student before registration opens? | No — it lifts only the closing deadline. Before `registration_opens_at` everything is denied. | Early registration is a different privilege from late correction; fail closed until a real requirement shows up. |
| A16 | 2026-07-15 | Who corrects a published grade, and can a published grade be re-entered as a draft? | Corrections are records-officer only, require a reason, and set state `amended`; draft entry refuses published/amended rows outright (409) for everyone including the officer. | A published grade is a fact students have seen — changing it must be an attributed, reasoned, history-preserving act, never a quiet re-save. The prior save_draft could silently unpublish; that hole is closed. |
| A17 | 2026-07-15 | Does `grade_entry_closes_at` bind everyone? What does NULL mean? | The window binds instructors only; the records officer may enter late (the escape hatch). NULL = no deadline configured = no window. | Late grades are a real administrative need; routing them through the officer keeps one accountable authority. NULL-means-closed would brick grading on every term seeded before the column was used. |
| A18 | 2026-07-15 | Where does revision history live? | A BEFORE UPDATE trigger copies the OLD grade row (value, state, author, version) into `grade_revision`; DELETE on grade_record is refused by trigger. | The history invariant must hold for every path — service, script, psql — not just the paths that remember to write a history row. Same reasoning as the 0010 capacity trigger. |
| A19 | 2026-07-15 | May any user id be assigned as a section instructor? | No — the target must hold the `instructor` role in the institution (else 422), and only registrar/institution_admin assign. | Grading power must flow through the role system; assigning a student as instructor by id typo would otherwise grant it silently. |
| A20 | 2026-07-16 | The Phase 5 prompt requires "approval/rejection with a required reason" — did that mean both decisions or only rejection (as originally implemented)? | Both: an approval also refuses a blank reason (422). | Issuing an official document is the sensitive act; the decision trail should say why in both directions. Conservative reading of the prompt; one `trim().is_empty()` check to relax. |
| A21 | 2026-07-16 | How stale is an orphaned job? | `APP_JOB_STALE_SECS`, default 300s — 60× the demo render time; reaped attempts count against the same budget of 3 as render failures. | A crash-looping worker must not retry forever any more than a failing render; five minutes cannot mistake a slow-but-alive render for a dead worker at demo document sizes. Deployments with heavyweight templates raise the knob. |
| A22 | 2026-07-16 | May `/license/import` be anonymous? The signature alone proves platform authority, but `license_change.changed_by_user_id` and `audit_event.actor_user_id` are NOT NULL by design. | Import requires an authenticated `institution_admin` or `platform_licensing_admin` (401/403 otherwise). Login is license-exempt, so a locked deployment can still recover: sign in, then import. | Fail closed and keep the audit trail honest — a recovery action with no attributable actor would be the only unaudited sensitive mutation in the system. Cheap to relax if a headless recovery path is ever demanded. |
| A23 | 2026-07-16 | Do holidays/events affect policy (deadlines, registration windows)? Not specified. | No — events are calendar data only; term dates alone govern policy windows. | Conservative: an admin adding a holiday must not silently move an academic deadline. If holiday-aware deadlines become a requirement, they get their own explicit rule and tests. |
| A24 | 2026-07-16 | Default state for per-institution document types, and effect on in-flight requests? | All three types enabled on migration and for new institutions (trigger); disabling blocks NEW requests only (fail closed on missing rows), pending/approved requests continue. | Preserves existing behavior on upgrade; cancelling in-flight officer-approved work because of a config toggle would destroy state the officer already vouched for. |
| A25 | 2026-07-20 | Where does `frontend/` live when the git repo root was `backend/`? | Moved the repository root to the project root (pure rename, history preserved); ADR-12. | The phase brief requires committing frontend/ work; a second repo would split one system's history. Cheap to reverse (move `.git` back). |
| A26 | 2026-07-20 | May the backend build depend on Node? | No: `frontend/dist/` is committed; CI rebuilds and diffs it. | CLAUDE.md keeps one Rust binary buildable from a bare checkout; a reproducible-diff step keeps committed artifacts honest. |
| A27 | 2026-07-20 | "Full section with a waitlist" in the demo seed — but no waitlist feature exists anywhere in the backend. | Seeded the full section without waitlist rows; flagged in the session report rather than inventing a schema mid-slice. | A waitlist is a real feature (schema + policy + UI), not seed data; conjuring rows for a nonexistent table would fake a capability. |
| A28 | 2026-07-20 | Should signed-out API clients also be redirected to sign-in? | Only browser GETs under `/ui/` redirect (303); API routes keep the JSON 401. | Programmatic clients need honest status codes; humans need to land on the sign-in page. |

(The Phase 7 license file format is frozen as format v1 — ADR-10.)
