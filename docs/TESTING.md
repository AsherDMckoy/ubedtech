# Testing

## The four gates (CLAUDE.md §6)

Run after every slice; all must pass before a commit:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

CI (`.github/workflows/ci.yml`) runs the same gates plus a release-profile
build against a PostgreSQL 16 service.

**Never pipe a gate into `tail`/`grep` inside a `&&` chain** — the chain
sees the pipe's exit code, not the gate's, and a red gate slides straight
into the commit. This has bitten twice (Phases 5 and 7). Run the gate
bare, or end the pipeline with `; echo EXIT=$?` and read it.

## Database tests

`#[sqlx::test(migrations = "./migrations")]` creates a fresh throwaway
database per test from `DATABASE_URL`, runs all migrations from empty, and
tears it down. Local runs need PostgreSQL and the `DATABASE_URL` from `.env`.

**Local parallelism:** with 40+ database tests, unbounded test threads can
exhaust local PostgreSQL connections (`PoolTimedOut` in sqlx's testing
harness — an infrastructure flake, not a code failure). Run
`cargo test --all-targets --all-features -- --test-threads=4` locally;
CI's service container copes with the default.

## Current suite (128 tests as of Phase 8)

Phase 8 added: `storage::tests::reads_are_confined_to_the_storage_root`
(absolute + traversal paths refused) and
`oversized_request_bodies_are_refused` (413 at the 64 KiB bound), plus the
no-store default asserted in the header test and the immutable-asset test
now running under the same middleware. Load benchmarks live in `load/`
(not part of `cargo test`; see PERFORMANCE.md).

## Phase 7 (126 tests)

Phase 7 (frontend hardening) added 7 and strengthened the existing UI flow
tests: every critical page they fetch now passes the structural
accessibility audit (`shared::assets::assert_page_a11y` — landmarks, one
h1, labels, table captions/scopes, no inline style/handlers). New tests:
asset serving (fingerprint + immutable cache), gzip compression, badge
color stability, CSS/JS size budgets, template scan (no images, no
external URLs, every page extends base), WCAG contrast of the design
tokens, and document-request idempotency (concurrent + sequential same-key
submissions return the original; the UI test double-submits the rendered
form). The manual accessibility checklist automation can't replace is in
`docs/FRONTEND_DESIGN_SYSTEM.md`.

## Phase 6 (119 tests)

Phase 6 added 10: `institution` (events/holidays admin-only + scoped +
audited, settings + document-type config validation and auditing, the
fail-closed disabled-type proof, the calendar UI flow, and the explicit
`institution_admin_does_not_bypass_domain_rules` proof), `licensing` (the
institution-wide lock test proving nobody is suspended, the half-open
validity-window boundary, and the signed-license import pair — a valid
import unlocks end to end; tampered/foreign-key/wrong-deployment/expired/
misdirected/unknown-format files are all rejected with nothing written),
and `config` (public-key parsing). The Ed25519 signing key exists only in
`licensing::tests::import`.

## Previous phases (still green)

Phase 5 added 9 `documents` tests: crash recovery via the reaper (commit
'running', reap, requeue, live worker completes), terminal reaping,
live-job safety, two-worker SKIP LOCKED race, duplicate-job idempotency
(one artifact ever), bounded retries with recorded reasons,
approval/rejection atomicity + authorization, download authorization +
checksum verification, and the full request→review→generate→download UI
flow. It also fixed the intermittent idempotency-test failure previously
misattributed to local pool exhaustion: the enrollment receipt carried a
Rust-side nanosecond timestamp that PostgreSQL truncates to microseconds;
receipts now use `INSERT … RETURNING`. (`PoolTimedOut` under unbounded
parallelism remains real — keep `--test-threads=4` locally.)

Phase 4 (100 tests) added 8 `records` tests: correction history (prior value + author
preserved), entry-window enforcement, crafted-request roster/grading
scoping, instructor-assignment validation, snapshot immutability +
versioning + published-only content, academic history, and the full
three-role UI flow (instructor enters → officer publishes → student sees
published only).

Phase 3 (92 tests) added 25: `academics` (policy unit + role matrix,
institution scoping across two institutions, unique-code conflicts,
transactional capacity, meeting/prerequisite validation, catalog),
`enrollment` (deadline consolidation, distinct missing-capacity failure,
single-use overrides incl. capacity and deadline, holds, idempotent
resubmission, duplicate enrollment, schedule conflicts, prerequisites,
duplicate-drop and drop-vs-add races) and `ui` (full plain-form
login→catalog→register→drop flow; all six rejection cases rendering
inline feedback). The seat-race test remains the anchor.

Phase 2 added 45 tests across `identity_access` (password unit tests,
session-store sqlx tests, HTTP lifecycle/CSRF/throttle/rotation/role tests,
policy unit tests, bootstrap tests) and `licensing` (exemption unit tests,
402 lock/unlock end-to-end). The full role × operation mapping is in
`docs/PERMISSIONS.md` — every matrix row cites its proving tests.

Baseline suite from Phases 0–1:

- `enrollment::tests::only_one_student_gets_the_last_seat` — races two
  registrations for one remaining seat over real PostgreSQL and asserts
  exactly one success and `enrolled_count = 1`. **This is the architecture's
  proof; it must stay green in every phase that touches enrollment.**
- `config::tests` (8) — defaults, validation failures, environment parsing,
  and that config error text never echoes values.
- `shared::error::tests` (4) — database error text (SQL, table names) never
  reaches an HTTP body; status-code map; user-facing messages survive.
- `shared::observability::tests` (5) — request-id validation, echo of valid
  inbound ids, replacement of hostile ones, generation.
- `app::tests` (4) — liveness without state, readiness reflecting the cached
  flag, full security-header set with Alpine-CSP-compatible CSP, HSTS
  production-only.

## Conventions

- Unit tests live in a `#[cfg(test)] mod tests` beside the code; policy and
  pure functions get plain `#[test]`s with no database.
- Import actix's test helpers as `use actix_web::test as actix_test;` —
  importing `test` directly shadows the `#[test]` attribute.
- Integration tests that need the schema use `#[sqlx::test]`; never point
  tests at a shared long-lived database.
- Every phase adds its acceptance tests per `IMPLEMENTATION_PLAN.md`; the
  role × operation matrix started with Phase 2 (`docs/PERMISSIONS.md` —
  rows only exist when test-backed) and is completed in Phase 8.
- HTTP tests build the app with the same middleware registration order as
  `main.rs` (see the `test_app!` macros); if `main.rs` ordering changes,
  change the macros with it.
