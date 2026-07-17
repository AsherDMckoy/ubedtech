# Backup and restore

What must be backed up, how, and the rehearsal proving the procedure works.
A backup that has never been restored is a hope, not a backup — re-run the
rehearsal below after schema changes that add tables holding institutional
state, and at least once per term in production.

## What a backup consists of

1. **The PostgreSQL database** — all institutional state: accounts, roles,
   enrollment, grades, snapshots, document requests, licensing, audit.
   `pg_dump -Fc` (custom format: compressed, parallel-restorable,
   selective).
2. **The document artifact store** (`APP_DOCUMENT_STORAGE_PATH`, or the
   object-storage bucket in production) — generated PDFs. Artifacts are
   re-derivable from their immutable snapshots, but re-generation produces
   new files whose checksums won't match copies already delivered, so back
   the store up alongside the database. Files are content-hashed and
   immutable: an rsync/bucket-replication copy taken near the database
   dump cannot be internally inconsistent; a file for a not-yet-committed
   row is unreferenced and harmless, in either direction of skew.
3. **Configuration** — the environment (see `.env.example` for the full
   list). Secrets (`DATABASE_URL`, `APP_LICENSE_PUBLIC_KEY`) come from the
   deployment's secret store, never from a file in the backup itself.

Encrypt backups at rest (the design brief requires it; `pg_dump | age` or
encrypted object storage both work) and store them off the host.

## Procedure

```sh
# Backup (run as a role that can read all tables):
pg_dump -Fc -f ubedtech-$(date +%F).dump "$DATABASE_URL"
# + copy/replicate the artifact store + record the config/env versions.

# Restore to a fresh database:
createdb ubedtech_restored
pg_restore -d ubedtech_restored --no-owner ubedtech-YYYY-MM-DD.dump
# Point DATABASE_URL at the restored database and start the binary:
# startup runs migrations (no-ops when current) and refuses to start
# without a valid license row — a truncated dump fails loudly here.
```

Verify after every restore, minimum: row counts of `audit_event`,
`enrollment`, `grade` tables against the source; `/health/ready` 200;
`/license/status` shows the expected license; one real authenticated read.

## Rehearsal record — 2026-07-17 (Phase 8)

Performed on the load-test dataset (14 MB database: 200 sections, 3,887
document requests, 3,886 audit events, 1 session; PostgreSQL 18.4):

| Step | Result |
|---|---|
| `pg_dump -Fc` | 0.2 s, 638 KB dump |
| `pg_restore` into a fresh database | 15.4 s, no errors |
| Row-count comparison (document_request, section, audit_event, user_session) | identical source ↔ restored |
| Release binary started against the restored database | migrations no-op, license row loaded, `/health/live` and `/health/ready` 200 |
| `/license/status` | active, correct validity window |
| Fresh login + authenticated catalog read | 200, byte-identical response size to the source database |

One integrity observation from the comparison: exactly one
`document_request` row without its `document.requested` audit event — it
was the raw `psql INSERT` used to measure fsync latency during the load
run, not an application write. Every application-created row has its
same-transaction audit row; the check
(`document_request` without matching `audit_event`) is a useful
post-restore verification and is recorded above for future rehearsals.
