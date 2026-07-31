# University of Belize education platform

Rust/Actix/PostgreSQL backend (`backend/`) + server-rendered frontend
assets and templates (`frontend/`). Read `CLAUDE.md` (engineering
constitution) and `FRONTEND.md` (frontend constitution) before changing
anything; per-area docs live in `backend/docs/`.

## Build

Backend (no Node required — `frontend/dist/` is committed, ADR-12):

```sh
cd backend
cargo build --release
cargo test --all-targets --all-features -- --test-threads=4
```

`cargo test` needs `DATABASE_URL` too (see below): the database tests use
`#[sqlx::test]`, which creates a throwaway database per test from that URL.

Frontend assets (only when `frontend/styles/` or `frontend/js/` change;
Node ≥ 26):

```sh
cd frontend
npm ci
npm run build   # writes fingerprinted bundles into frontend/dist/ — commit them
npm test        # accessibility harness (axe) over rendered critical pages
```

## Run locally

**1. PostgreSQL.** Either a local install:

```sh
createdb ubedtechdb        # must match the database name in DATABASE_URL
```

or Docker (creates the database for you, persists across restarts):

```sh
docker run --name ubedtech-pg \
  -e POSTGRES_PASSWORD=ubedtech \
  -e POSTGRES_DB=ubedtechdb \
  -v ubedtech-pgdata:/var/lib/postgresql/data \
  -p 5432:5432 -d postgres:17
```

**2. Configure.**

```sh
cd backend
cp .env.example .env
```

Set `DATABASE_URL` in `.env` to match step 1:

- local install (peer auth as your OS user): `postgresql://localhost:5432/ubedtechdb`
- Docker above: `postgresql://postgres:ubedtech@localhost:5432/ubedtechdb`

**3. Migrate.**

```sh
cargo run
```

Migrations apply automatically at startup. On an empty database the
server then **exits** with `valid institution license required before
startup` — that is expected: the schema now exists but no institution/
license row does yet. The seed provides both.

**4. Seed** (from `backend/`, same URL as `DATABASE_URL` — `.env` is
read by the server, not your shell):

```sh
psql postgresql://localhost:5432/ubedtechdb -f src/dev/seed.sql
# Docker: docker exec -i ubedtech-pg psql -U postgres -d ubedtechdb < src/dev/seed.sql
```

**5. Run.**

```sh
cargo run
```

Open <http://localhost:8080/ui/login>. Demo accounts and passwords are
listed in the header of `src/dev/seed.sql` (development only).

## Demo seed

`seed.sql` (step 4 above) seeds the University of Belize institution, an
active license, a development term, and demo accounts/data for every
critical screen. Idempotent; re-run freely.

For a realistic large dataset on top of that (UI evaluation at true
scale):

```sh
cargo run -- seed-demo
```

Deterministic (fixed RNG seed): ~900 students, 45 instructors, ~85
courses across five faculties, four terms (two fully graded), ~14,000
enrollments and grades made through the real services so every counter
and audit invariant holds (a verification pass fails loudly otherwise),
~50 document requests across all six states, holds, calendar events, a
second institution for scoping checks, and deliberate layout stressors
(long names, an over-long course title, empty-state accounts). Skips
itself if already present; refuses `APP_ENV=production`. Rebuild = drop
the database, re-apply `seed.sql`, re-run. The `seed.sql` core scenarios
(the rehearsed demo in `backend/docs/DEMO_SCRIPT.md`) are preserved
exactly.

Load-test dataset on top of that: `backend/load/README.md`.

## Releases

Pushing a version tag builds Linux and Windows binaries and attaches
them to a GitHub Release (`.github/workflows/release.yml`):

```sh
git tag v0.1.0
git push origin v0.1.0
```

Each archive contains the server binary, the `dist/` frontend assets,
the demo `seed.sql`, and `RUN.md` — the standalone instructions for
running from a release package with a Docker PostgreSQL.
