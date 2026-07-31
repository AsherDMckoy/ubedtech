# Load testing

Produces the three benchmark-class reports CLAUDE.md §4 requires (results
+ metadata live in `docs/PERFORMANCE.md`). Tool: `wrk`.

```sh
# 1. Dedicated throwaway database, seeded. The URL needs an explicit
#    username — without one sqlx falls back to a default role, not the
#    OS user the way psql does.
createdb ubedtech_load
DATABASE_URL=postgresql://$USER@localhost:5432/ubedtech_load ./target/release/backend  # runs migrations, exits: no license yet
psql -d ubedtech_load -f src/dev/seed.sql
psql -d ubedtech_load -f load/seed.sql

# 2. Release server against it (readiness on /health/live).
DATABASE_URL=postgresql://$USER@localhost:5432/ubedtech_load \
  APP_BIND_ADDR=127.0.0.1:8087 APP_DOCUMENT_STORAGE_PATH=/tmp/load-docs \
  RUST_LOG=warn ./target/release/backend &

# 3. Session for the authenticated classes.
curl -s -X POST http://127.0.0.1:8087/api/v1/session/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"dev.student","password":"load-test-password-123"}' -i
# → export COOKIE="ub_session=<cookie>" CSRF="<csrf_token>"

# 4. The three classes.
wrk -t8 -c64 -d30s --latency http://127.0.0.1:8087/health/live      # A: in-process
wrk -t8 -c64 -d30s --latency -H "Cookie: $COOKIE" \
  "http://127.0.0.1:8087/api/v1/catalog?term_id=00000000-0000-0000-0000-000000000005&page=0"  # B: read path
wrk -t4 -c16 -d30s --latency -s load/documents.lua http://127.0.0.1:8087   # C: durable writes
```

Verify class C actually wrote: `select count(*) from document_request`
must match the request count. (First run of this suite didn't — wrk seeds
every thread's RNG with `os.time()`, so all threads generated the same
idempotency keys and the server correctly deduplicated them; documents.lua
now seeds per thread. Good accident: idempotency held under load.)

## Registration writes (`register_drop.lua`)

Each thread alternates register → drop on its own section — every
request a committed enrollment transaction. Requirements the script
cannot check for you (each was discovered as a wall of honest 409s):
run with `-t N -c N` (state is per thread-VM); use N DISTINCT students
(the service serializes per student); pair each student with a section
that has seat headroom and does not conflict with their existing
schedule; the term's registration window must be open. Pass one
cookie/csrf/section per thread:

```sh
COOKIES="ub_session=a,ub_session=b,..." CSRFS="t1,t2,..." SECTIONS="s1,s2,..." \
  wrk -t16 -c16 -d30s --latency -s load/register_drop.lua http://127.0.0.1:8087
```

Verify: `audit_event` count delta must equal the request count (adds +
drops), and the enrollment delta the add half. 2026-07-30 measured
117.8 committed ops/s at t16/c16 — numbers in docs/PERFORMANCE.md.
