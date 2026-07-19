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

Load-test dataset on top of that: `backend/load/README.md`.
