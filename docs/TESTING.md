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

## Database tests

`#[sqlx::test(migrations = "./migrations")]` creates a fresh throwaway
database per test from `DATABASE_URL`, runs all migrations from empty, and
tears it down. Local runs need PostgreSQL and the `DATABASE_URL` from `.env`.

## Current suite (22 tests)

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
  role × operation matrix tests land with `docs/PERMISSIONS.md` in Phase 8.
