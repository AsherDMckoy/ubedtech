# University Education Platform — Claude Code Project Rules

This file is read automatically at the start of every Claude Code session in this
repository. It contains the rules that must hold across ALL phases of work, not
just the current one. Phase-specific instructions come from a separate prompt
pasted at the start of each session — this file is the constitution, that prompt
is the current assignment.

If anything in a phase prompt conflicts with this file, this file wins unless the
phase prompt explicitly says it is overriding a specific rule and why.

---

## 0. Non-negotiable architecture

- One root `Cargo.toml` inside `backend/`. One Actix Web binary. One PostgreSQL
  database. No Cargo workspace merely to simulate modularity. No microservices.
  No internal HTTP calls between feature areas. No event bus unless a measured,
  unavoidable requirement later justifies it and is recorded in an ADR.
- Feature separation is directories + Rust modules + private implementation
  files + typed service APIs + explicit data ownership — not package boundaries.
- Feature directories under `backend/src/`: `identity_access/`, `institution/`,
  `academics/`, `enrollment/`, `records/`, `documents/`, `licensing/`, `jobs/`,
  `shared/`, plus `audit.rs`, `app.rs`, `config.rs`, `db.rs`, `main.rs`. This may
  evolve when implementation reveals a clearer seam — record why in an ADR.
- Never create `utils/`, `managers/`, `common_services/`, `generic_repositories/`,
  `helpers/`, or any other dumping ground.
- A feature directory earns its files; don't force every feature into the same
  template. Typical shape when warranted: `mod.rs`, `http.rs`, `service.rs`,
  `policy.rs`, `queries.rs`, `types.rs`, `tests.rs`.
- Actix types stay at the HTTP boundary. Services and policy modules use
  ordinary Rust types so they're testable without spinning up the framework.
- No generic `Repository<T>`. Introduce a trait only at a real replaceable
  boundary: artifact storage, email delivery, clocks, randomness for
  deterministic tests, external identity providers, external university
  integrations. Nowhere else.

## 1. Known defects to fix, not rediscover

These were found in a prior review of the design docs this codebase implements.
Do not re-derive them from scratch — fix them as part of the relevant phase, and
write the corresponding test:

1. **Auth/session/CSRF/license middleware is specified but not wired.** The
   `Actor` extractor reads from request extensions, but nothing populates them —
   no session-resolution middleware exists or is `.wrap()`'d in `main.rs`.
   `LicenseGate::require_active` exists but is never called on a request path.
   `argon2` and `subtle` are dependencies with no calling code. This is the
   single highest-priority gap — Phase 2 does not exist until this is closed
   and tested (a request without a valid session must 401; a request to a
   locked institution must 402).
2. **SQLx feature flag is wrong.** `runtime-tokio-rustls` does not exist as a
   single feature in SQLx 0.8+/0.9. Use `runtime-tokio` plus a separate
   `tls-rustls-*` feature (e.g. `tls-rustls-ring-webpki`).
3. **No reaper for orphaned `document_job` rows stuck in `'running'`.** If the
   worker process dies mid-render, the job never returns to `'queued'`. Add a
   periodic sweep using the existing `locked_at`/`locked_by` columns.
4. **`section_capacity` is not guaranteed to exist for every `section`.** A
   missing capacity row and a genuinely full section both currently produce the
   same "section is full" error. Create the capacity row transactionally with
   the section (or via trigger/constraint), and add a test that registering
   against a section with no capacity row fails loudly and distinctly.
5. **Add-deadline vs. drop-deadline policy is unresolved.** Registration checks
   `registration_closes_at`; drops check `drop_add_closes_at`. Confirm with
   yourself (record as an assumption + ADR if unresolved) whether a student
   should be able to *add* a class during the drop/add window after
   `registration_closes_at`. Pick a conservative default, make it configurable,
   write it down.
6. **Dead capacity-override branch.** The override check for `"capacity"`
   currently runs a query whose result never changes the outcome — both
   branches return an error. Either implement the override for real (with the
   full override record: who, why, what rule, expiry, which enrollment
   consumed it) or delete the query and the branch.

Treat this list as seed input to `docs/CURRENT_STATE.md`, not as the complete
list — inspection may surface more.

## 2. Security baseline (every phase, not just Phase 2)

- No JWTs by default. Only if a real external-client boundary requires them,
  and only with an ADR.
- Opaque, high-entropy server-side session tokens; store only a cryptographic
  hash of the token in PostgreSQL, never the token itself.
- `HttpOnly` always; `Secure` in production; explicit `SameSite`.
- Session rotation after authentication and after privilege changes. Explicit
  idle and absolute expiration. Revocation on logout, suspension, and password
  reset.
- Argon2id, parameters documented and configurable. Login throttling that does
  not itself become an account-lockout denial-of-service vector.
- CSRF protection on every browser state-changing request — real middleware,
  not a comment saying middleware should check this.
- Authorization lives in services, not templates. Button visibility is not
  authorization — enforce with tests, not trust.
- Institution scoping (`institution_id`) in every relevant query AND in the
  relevant unique constraints, not just the WHERE clause.
- Parameterized SQL only. No raw SQL errors, stack traces, or secret
  configuration ever reach an HTTP response or a log line. Redact PII in logs.
- The development actor bypass (if one exists in the current codebase) must be
  either removed or gated behind a cfg that cannot compile into a release
  binary — not an env var check at runtime.
- Audit records for sensitive state changes are written in the SAME database
  transaction as the state change they describe. Not after. Not best-effort.

## 3. Correctness rules that apply everywhere

- Preserve correctness across: retries, duplicate requests, concurrent
  requests, and process crashes. If a code path can't survive a crash between
  its two halves, it's not done — it needs an idempotency key, a job row, or a
  transaction boundary that makes the crash safe.
- All timestamps stored in UTC. Institution timezone governs display and
  policy enforcement (deadlines, windows), never storage.
- Every invariant the database can express as a constraint should be a
  constraint, not just an application check — the application check runs
  first for good error messages, the constraint is the actual guarantee.
- Transaction ownership belongs to the service method implementing the use
  case. Never open a second transaction to "fix" what a query pattern should
  have solved instead.

## 4. Performance rules

- Three separate benchmark classes exist and are never conflated: in-process
  gates/local responses, read paths, durable transactional paths. Report each
  separately, always with hardware, dataset size, concurrency, request mix,
  cache state, durability behavior, response sizes, p50/p95/p99, throughput,
  error rate.
- Optimization order: (1) clearest correct solution, (2) correct algorithm/
  query/transaction/data shape, (3) remove measured pessimization, (4)
  infrastructure — only after profiling proves code and schema are already
  sound. Do not add infrastructure to compensate for a bad query or a wrong
  transaction boundary.
- No N+1. No `SELECT *` on hot paths. Pagination for unbounded lists. No
  blocking filesystem or CPU-heavy work (PDF generation included) on an Actix
  worker thread — it belongs in the job worker.
- A bounded PostgreSQL pool. Bounded background-worker concurrency. No global
  mutex on a request path.

## 5. Testing discipline

- Unit tests for policy functions, state transitions, permission checks,
  license decisions, time-window behavior — no database required.
- Integration tests against a real PostgreSQL test database: migrations from
  empty, constraints, concurrency (registration and drop races proving seats
  cannot be oversold), job claiming and retries, institution scoping,
  idempotency, rollback.
- HTTP integration tests: full auth lifecycle, session lifecycle, CSRF success
  and failure, role and resource authorization, licensing lock behavior,
  security headers, body-size limits.
- A concurrency test that races two registrations for the last seat and
  asserts exactly one success is not optional — it is the proof the
  architecture works. Every phase that touches enrollment must keep this test
  green.
- `docs/PERMISSIONS.md` contains a full role × operation matrix, and every cell
  in that matrix is backed by a test, not just documentation.

## 6. Session and commit discipline (READ THIS BEFORE STARTING ANY PHASE)

- Each Claude Code session works on exactly ONE phase, as named in the prompt
  pasted at session start. Do not start the next phase's work in the same
  session even if there is time/budget left — stop, report status against that
  phase's acceptance criteria, and end the turn.
- After each vertical slice within a phase (a slice = migration + types +
  policy + service + HTTP adapter + template/fragment + tests, wired end to
  end — not a handler stub):
  1. Run `cargo fmt --check && cargo clippy --all-targets --all-features -- -D
     warnings && cargo test --all-targets --all-features`.
  2. Fix anything that fails before moving on. Never proceed with a red gate.
  3. `git add` and `git commit` with a message naming the slice and what it
     makes true that wasn't true before (e.g. `feat(enrollment): atomic seat
     reservation with concurrency test`). One slice, one commit, small diffs.
  4. Only then start the next slice.
- Never produce one giant end-of-session diff. If a session ends with an
  uncommitted, unreviewable pile of changes, that session failed regardless of
  what the code does.
- Never mark something done in `docs/IMPLEMENTATION_PLAN.md` because a handler
  exists. Mark it done because the acceptance test for that slice is green and
  committed.

## 7. Handling unknown business policy

- Claude Code cannot pause mid-session to ask a business question and get a
  synchronous answer. So: when a decision depends on business policy that
  isn't in the docs or this file, do NOT block. Instead:
  1. Choose the most conservative default (the one that denies access, fails
     closed, or preserves an academic-integrity invariant rather than user
     convenience).
  2. Make it configurable rather than hardcoded, where that's cheap.
  3. Write the assumption, the default chosen, and why, into
     `docs/IMPLEMENTATION_PLAN.md` under "Assumptions" AND flag it at the top
     of the session's final report to the human.
  4. Keep moving.
- The one time to actually stop and end the turn asking a question: if
  proceeding would require guessing something that is expensive or unsafe to
  reverse later (e.g. the on-disk license file format, the primary key
  strategy for a table already seeded with data). Everything else: assume,
  document, continue.

## 8. Deviating from the design docs

`UB_EDTECH_SYSTEM_DESIGN_AND_ARCHITECTURE.md` and
`UB_EDTECH_IMPLEMENTATION_GUIDE_WITH_CODE.md` are strong guidance, not
infallible specs. When you deviate, add a numbered entry to
`docs/ARCHITECTURE_DECISIONS.md` with: the original decision, the replacement,
why it's better, migration/compatibility consequences, and which test proves
the replacement works. A deviation without an ADR entry is not permitted to
land in a commit.

## 9. Absolute prohibitions

No `todo!()` or `unimplemented!()` on any path a real request can reach. No
panic-based request handling. No fake success responses — especially for
registration, grades, and official documents, where a false positive is worse
than an honest error. No development auth reachable from a release build. No
`#![allow(dead_code)]` / `#![allow(unused)]` blanket suppressions — either wire
the code up, delete it, or write a narrow `#[allow(...)]` with a one-line
reason attached to the specific item.

## 10. Documentation that must stay current

`docs/CURRENT_STATE.md`, `docs/IMPLEMENTATION_PLAN.md`,
`docs/ARCHITECTURE_DECISIONS.md`, `docs/PERMISSIONS.md`, `docs/SECURITY.md`,
`docs/OPERATIONS.md`, `docs/BACKUP_AND_RESTORE.md`, `docs/PERFORMANCE.md`,
`docs/FRONTEND_DESIGN_SYSTEM.md`, `docs/API.md`, `docs/TESTING.md`. Update the
ones relevant to the current phase before ending the session — not all ten
every time, but never leave one silently stale either.
