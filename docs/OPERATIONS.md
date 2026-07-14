# Operations

Status: Phase 2. Single binary, single PostgreSQL database.

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
| `APP_WORKER_ID` | `document-worker-1` | claims jobs under this name (`document_job.locked_by`) |
| `APP_SHUTDOWN_TIMEOUT_SECS` | 30 | HTTP drain and worker-stop budget |
| `APP_READINESS_INTERVAL_SECS` | 5 | readiness prober cadence |
| `RUST_LOG` | `info,actix_web=info,sqlx=warn` | tracing filter (EnvFilter syntax) |

## Startup sequence

1. Load `.env` (dev only), install JSON tracing.
2. Parse + validate config; abort on error.
3. Connect bounded pool; run migrations (`migrations/`, embedded).
4. Load the institution license snapshot; **refuse to start** if no
   `institution_license` row exists (fail closed).
5. Spawn document worker and readiness prober; serve HTTP.

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
stays `running` until the Phase 6.1 reaper requeues it — until that lands,
check `document_job` for stale `running` rows after a crash:

```sql
SELECT id, request_id, locked_by, locked_at FROM document_job
WHERE status = 'running' AND locked_at < now() - interval '10 minutes';
```

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
