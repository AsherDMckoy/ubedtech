# Operations

Status: Phase 2. Single binary, single PostgreSQL database.

**Deployment topology: the binary and PostgreSQL run on the same host**
(or, at minimum, a same-datacenter private network with sub-millisecond
RTT). Measured, not assumed: a request makes ~4–6 sequential database
round trips, so at 53 ms app↔DB RTT the catalog p50 went from 3.7 ms to
324 ms and throughput fell 96 % with both machines idle
(PERFORMANCE.md, remote-database topology measurement, 2026-07-29).

## Configuration

All configuration is environment variables, parsed and validated at startup
by `src/config.rs` (startup aborts on invalid values; error messages never
echo the configured value). `.env` is loaded in development only and never
overrides real environment variables. See `.env.example` for the full list.

| Variable | Default | Meaning |
|---|---|---|
| `APP_ENV` | `development` | `production` enables HSTS; everything else behaves as development |
| `DATABASE_URL` | — (required) | PostgreSQL connection string; may embed credentials — never logged |
| `APP_BIND_ADDR` | `0.0.0.0:8080` | listen address |
| `APP_DB_MAX_CONNECTIONS` / `APP_DB_MIN_CONNECTIONS` | 64 / 8 | bounded pool = deliberate backpressure |
| `APP_DB_ACQUIRE_TIMEOUT_SECS` | 5 | fail fast when the pool is saturated |
| `APP_DOCUMENT_STORAGE_PATH` | `./var/documents` | PDF artifact store (back up with the database) |
| `APP_FRONTEND_DIST` | `../frontend/dist` (next to the crate) | Built, fingerprinted frontend bundles; loaded once at startup, which fails loudly if they are missing (`npm run build`) |
| `APP_WORKER_ID` | `document-worker-1` | claims jobs under this name (`document_job.locked_by`) |
| `APP_SHUTDOWN_TIMEOUT_SECS` | 30 | HTTP drain and worker-stop budget |
| `APP_READINESS_INTERVAL_SECS` | 5 | readiness prober cadence |
| `APP_JOB_STALE_SECS` | 300 | dead-worker reap threshold for `running` document jobs |
| `APP_LICENSE_PUBLIC_KEY` | unset | self-hosted only: Ed25519 public key (hex) for signed license imports; unset = imports refused |
| `RUST_LOG` | `info,actix_web=info,sqlx=warn` | tracing filter (EnvFilter syntax) |

## Startup sequence

1. Load `.env` (dev only), install JSON tracing.
2. Parse + validate config; abort on error.
3. Connect bounded pool; run migrations (`migrations/`, embedded).
4. Load the institution license snapshot; **refuse to start** if no
   `institution_license` row exists (fail closed).
5. Spawn document worker and readiness prober; serve HTTP.

## Containers (demo stack)

`./demo.sh` (repo root) builds and launches the app + PostgreSQL with
podman or docker via `compose.yaml` + `backend/Containerfile`, waits,
smoke-checks a real sign-in, and prints the demo credentials.
`./demo.sh fresh` wipes the volume for a known-good stage;
`./demo.sh down` stops the stack.

- The image carries the release binary, the committed `frontend/dist`
  (`APP_FRONTEND_DIST=/app/dist`), and a writable `/app/var/documents`;
  it runs as a non-root user.
- The entrypoint runs `backend seed-demo` BEFORE serving — order is
  load-bearing: startup fails closed without an `institution_license`
  row, and seed-demo (which self-applies `seed.sql` on an empty
  database) is what creates it. The seed is idempotent, so restarts are
  a fast no-op.
- `seed-demo` refuses `APP_ENV=production`; this stack is for demos and
  development, not deployment.

## First platform administrator (bootstrap)

The `platform_licensing_admin` role cannot be granted through the HTTP API
(institution admins manage only institution roles). The first — and only —
platform admin is created by an operator on the host, before or after the
server is running:

```sh
# Recommended: pipe the password (twice: password, then confirmation) from
# a secret store so it never lands in argv, the environment, or shell
# history. `backend` is the compiled binary; DATABASE_URL must be set.
systemd-ask-password "platform admin password:" | { read -r pw; printf '%s\n%s\n' "$pw" "$pw"; } \
  | backend bootstrap-platform-admin ops.admin ops@example.edu
```

Usage: `backend bootstrap-platform-admin <username> <email>
[institution-code]` — the institution code is only needed if more than one
institution exists. Rules enforced by the command:

- Refuses (exit 1) if **any** platform licensing admin already exists;
  there is no code path that mints a second one. Recovering from a lost
  platform-admin credential is a manual, audited database operation.
- The password comes from stdin only (first line password, second line
  confirmation) and must meet the same 12-character minimum as every
  other path. Running it on a terminal warns that input will echo.
- Works on an unlicensed or locked deployment (it runs before the license
  check), because unlocking a locked deployment is exactly what the
  account is for.
- Account, credential, role, and audit record
  (`identity.platform_admin_bootstrapped`) are written in one transaction.

## Health endpoints

- `GET /health/live` → 200 `live`. Process responsiveness only; never
  consults dependencies. Use for restart decisions.
- `GET /health/ready` → 200 `ready` / 503 `not ready`. Reads a cached flag
  the prober refreshes every `APP_READINESS_INTERVAL_SECS` (DB `SELECT 1`
  bounded by the same interval). Use for traffic routing. Probes cost no
  database work regardless of frequency.

## Shutdown

SIGTERM/SIGINT → actix stops accepting, drains in-flight requests (up to
`APP_SHUTDOWN_TIMEOUT_SECS`), then the document worker is signaled and given
the same budget to finish its current job. A job interrupted by a hard kill
stays `running` only until the reaper requeues it (see "Orphaned document
jobs" below) — no manual intervention needed.

## Logs

JSON to stdout. Every request logs one completion line inside a span with
`request_id`, `method`, `path`, `status`, `duration_ms` — no query strings,
headers, cookies, bodies, or PII. Clients may pass `x-request-id`
(8–64 chars, `[A-Za-z0-9_-]`) to correlate; the id is echoed in the
response header.

## Dev database

`psql ubedtechdb` — migrations are applied automatically at startup;
`src/dev/seed.sql` seeds an institution, dev student, term, and an active
license for local work: `psql -d ubedtechdb -f src/dev/seed.sql`.

`cargo run -- seed-demo` layers a deterministic large dataset on top
(src/dev/seed_demo.rs): ~900 students, four terms, ~14k enrollments made
through the real services, all document states, a second institution.
Development only — it exits with an error under `APP_ENV=production`, and
its final verification pass fails the command if any seat counter, capacity
row, or audit trail is inconsistent. Re-running detects the dataset and
does nothing; rebuild by dropping the database, re-applying `seed.sql`,
and re-running. Uses `synchronous_commit=off` on its own pool for speed —
never on the application pool.

## Document artifact storage

Artifacts (generated PDFs) live behind the `DocumentStore` trait
(`src/documents/storage.rs`) — the artifact-storage boundary CLAUDE.md §0
sanctions a trait for. Two operations: `write(hash, bytes) -> storage_path`
(must be atomic) and `read(storage_path) -> bytes`.

**Development / single node:** `FilesystemDocumentStore` under
`APP_DOCUMENT_STORAGE_PATH` (default `./var/documents`). Content-hash
filenames sharded by hash prefix (`ab/abcdef….pdf`); writes are tmp+rename
so a reader never sees a partial file. Back this directory up with the
database (BACKUP_AND_RESTORE.md); artifacts are re-derivable from
snapshots, but re-generation invalidates recorded checksums for signed
copies already delivered.

**Production (object storage):** implement `DocumentStore` against any
S3-compatible API:

- `write`: `PUT` to `documents/{hash[..2]}/{hash}.pdf` (single PUT is
  atomic in S3 semantics; multipart completes atomically too). Return the
  object key as the storage path. Enable bucket versioning + a deny-delete
  bucket policy to mirror the immutability the filesystem store gets from
  the database trigger + checksum verification.
- `read`: `GET` by key.
- The worker (`DocumentWorker<S: DocumentStore>`) and the download adapter
  are already generic/injected — swap the constructed store in `main.rs`,
  nothing else changes. Downloads re-verify sha256 against
  `generated_document.content_hash` on every read, so storage corruption
  or tampering is served as a 500, never as a document.

## Orphaned document jobs (the reaper)

A worker that dies mid-render leaves its job `running`. The worker loop
reaps jobs whose `locked_at` is older than `APP_JOB_STALE_SECS` (default
300) back to `queued` — or to terminal `failed` once the attempt budget
(3) is spent — at startup and every 60 seconds. Set the threshold well
above the longest legitimate render. `document_job.last_error` records
both render failures and reap events; a terminally failed job also fails
its request, visibly to the student and the officer queue.

## License operations

- **Hosted:** the platform licensing admin manages licenses at
  `/ui/platform/license` (status, history, reasoned suspend/activate).
  The page and its POST are license-exempt, so a locked deployment can
  always be unlocked: sign in (login is also exempt), open the panel.
  Disabling a license answers 402 institution-wide but suspends no
  accounts and revokes no sessions — reactivation restores service with
  everyone's sessions intact.
- **Self-hosted:** recovery is `POST /license/import` with a
  platform-signed license file (format, signing, and key rotation:
  docs/SECURITY.md). Configure `APP_LICENSE_PUBLIC_KEY` (public key only —
  never the signing key) and restart; an institution admin logs in and
  imports the file. Every import and status change lands in
  `license_change` plus an audit event, in the same transaction.
