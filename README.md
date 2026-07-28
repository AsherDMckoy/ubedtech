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

Frontend assets (only when `frontend/styles/` or `frontend/js/` change;
Node ≥ 26):

```sh
cd frontend
npm ci
npm run build   # writes fingerprinted bundles into frontend/dist/ — commit them
npm test        # accessibility harness (axe) over rendered critical pages
```

## Run locally

```sh
createdb ubedtech
cd backend
cp .env.example .env       # set DATABASE_URL, APP_DOCUMENT_STORAGE_PATH
cargo run                  # migrations run at startup
```

Startup refuses to serve without a valid institution license row — seed
first (below).

## Demo seed

```sh
psql "$DATABASE_URL" -f backend/src/dev/seed.sql
```

Seeds the University of Belize institution, an active license, a
development term, and demo accounts/data for every critical screen
(see the header of `seed.sql` for the account list and passwords —
development only, never production). Idempotent; re-run freely.

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
