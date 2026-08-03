# University of Belize education platform

A self-hostable university platform: registration and seat management,
schedules, grade entry and publishing, academic records, official
documents, holds and overrides, institution administration, and
licensing. One Rust/Actix binary, one PostgreSQL database,
server-rendered UI (`backend/` + `frontend/`).

Read `CLAUDE.md` (engineering constitution) and `FRONTEND.md` (frontend
constitution) before changing anything; per-area docs live in
`backend/docs/`.

---

## Quick start — one command (podman or docker)

```sh
./demo.sh
```

Builds the images, launches PostgreSQL + the app, migrates and seeds the
full demo dataset, waits until a real sign-in round-trips, and prints
the credentials. When it says READY, open
**<http://127.0.0.1:8080/ui/login>**.

| Command | What it does |
|---|---|
| `./demo.sh` | Build + launch + smoke-check (idempotent, safe to re-run) |
| `./demo.sh fresh` | Wipe the database volume first — known-good demo stage |
| `./demo.sh down` | Stop the stack (data kept) |

Requirements: `podman` + `podman-compose` (or `docker` with the compose
plugin) and `curl`. The first build compiles Rust in release mode — a
few minutes; re-runs take seconds. Stack definition: `compose.yaml` +
`backend/Containerfile`.

### Demo accounts

Every password is `ub-demo-password` (development seed only — never
production):

| Username | Person | Role |
|---|---|---|
| `demo.student` | Dana Castillo | Student, clean record |
| `demo.held` | Marlon Usher | Student with an advising hold |
| `demo.instructor` | Alba Flores | Instructor |
| `demo.registrar` | Renee Garbutt | Registrar + records + document officer |
| `demo.admin` | Iris Novelo | Institution admin |
| `demo.platform` | Platform Operations | Platform licensing admin |

A scripted, timed demo (three journeys with talking points and recovery
notes) lives in `backend/docs/DEMO_WALKTHROUGH.md`.

---

## Run bare metal (local development)

Prerequisites: Rust stable ≥ 1.88 (edition 2024), PostgreSQL (16 is
what the container stack ships; a recent local install works), and Node
≥ 26 **only if you will edit the frontend** — `frontend/dist/` is
committed (ADR-12), so backend work never needs Node.

**1. PostgreSQL.** Either a local install:

```sh
createdb ubedtechdb        # must match the database name in DATABASE_URL
```

or a container just for the database:

```sh
podman run --name ubedtech-pg \
  -e POSTGRES_PASSWORD=ubedtech \
  -e POSTGRES_DB=ubedtechdb \
  -v ubedtech-pgdata:/var/lib/postgresql/data \
  -p 5432:5432 -d docker.io/library/postgres:16
```

**2. Configure.**

```sh
cd backend
cp .env.example .env
```

Set `DATABASE_URL` in `.env` to match step 1:

- local install (peer auth as your OS user): `postgresql://localhost:5432/ubedtechdb`
- container above: `postgresql://postgres:ubedtech@localhost:5432/ubedtechdb`

`DATABASE_URL` is the only required setting; every other knob (bind
address, pool sizes, session lifetimes, Argon2 parameters, document
storage path, …) has a sane default and an `APP_*` override — the full
list is in `backend/src/config.rs` and `backend/docs/OPERATIONS.md`.

**3. Seed + migrate — one step.**

```sh
cargo run --release -- seed-demo
```

This connects, applies the embedded migrations, applies the core
`seed.sql` bootstrap itself if the database is empty (no psql needed),
and layers the full demo dataset. Idempotent — re-running is a fast
no-op. Takes ~40 s on first run.

**4. Run.**

```sh
cargo run --release
```

Open <http://localhost:8080/ui/login> and sign in with a demo account
from the table above.

Reset to a clean stage: drop and recreate the database, re-run step 3.
Note: the server intentionally **refuses to start** on a database with
no institution license row — step 3 is what creates it.

### Frontend changes

```sh
cd frontend
npm ci
npm run build   # fingerprinted bundles into frontend/dist/ — commit them
npm test        # renders sample pages; axe accessibility + jsdom behavior tests
```

The backend reads `frontend/dist` **at startup** (path override:
`APP_FRONTEND_DIST`); templates compile into the binary. After template
changes rebuild the backend; after CSS/JS changes rebuild dist and
restart.

### Tests and gates

```sh
cd backend
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features -- --test-threads=8
```

`cargo test` needs `DATABASE_URL` (the database tests use
`#[sqlx::test]`, which creates a throwaway database per test — the URL's
user needs `CREATEDB`). Cap `--test-threads` when PostgreSQL's
`max_connections` is at the default 100, or the suite can exhaust the
server and fail with `PoolTimedOut`.

## Troubleshooting

- **`./demo.sh`: no compose provider** — install `podman-compose`
  (Arch: `pacman -S podman-compose`) or use docker's compose plugin.
- **App container restarts once or twice on first boot** — normal while
  PostgreSQL initializes; the entrypoint retries behind a DB
  healthcheck and dumps logs if it gives up (~1 min).
- **"cannot read the frontend dist directory"** — run from the repo
  checkout, or set `APP_FRONTEND_DIST=/path/to/frontend/dist`.
- **"valid institution license required before startup"** — expected on
  an unseeded database; run `seed-demo` (it creates the license row).

## Demo seed details

`seed-demo` is deterministic (fixed RNG seed): ~900 students, 45
instructors, ~85 courses across five faculties, four terms (two fully
graded), ~14,000 enrollments and grades made through the real services
so every counter and audit invariant holds (a verification pass fails
loudly otherwise), ~50 document requests across all six states, holds,
calendar events, a second institution for scoping checks, and deliberate
layout stressors. Skips itself if already present; refuses
`APP_ENV=production`. The rehearsed core scenarios
(`backend/docs/DEMO_SCRIPT.md`) are preserved exactly.

Load-test dataset on top of that: `backend/load/README.md`.

## Releases

Pushing a version tag builds Linux and Windows binaries and attaches
them to a GitHub Release (`.github/workflows/release.yml`):

```sh
git tag v0.1.0
git push origin v0.1.0
```

Each archive contains the server binary, the `dist/` frontend assets,
the demo `seed.sql`, and `RUN.md` — standalone instructions for running
from a release package with a containerized PostgreSQL.

## Where to read next

- `backend/docs/DEMO_WALKTHROUGH.md` — podium-ready demo script
- `backend/docs/OPERATIONS.md` — configuration, startup, containers, backups
- `backend/docs/ARCHITECTURE_DECISIONS.md` — the ADR log
- `backend/docs/SECURITY.md` — sessions, CSRF, authorization model
