# Performance

## Frontend budgets (Phase 7, rebased on the frontend/ pipeline — ADR-12)

| Budget | Limit | Actual (2026-07-28, demo-polish session) | Enforced by |
|---|---|---|---|
| Stylesheet bundle (tokens + base + components) | ≤ 32 KiB uncompressed | 24.7 KiB (5.6 KiB gzipped) | `asset_sizes_stay_inside_the_budget` |
| Script bundle (Alpine CSP build + enhancements) | ≤ 80 KiB uncompressed | 64.3 KiB (21.1 KiB gzipped) | same |
| Sign-in visual | image-free | pure-CSS gradient panel — 0 image bytes, no layout shift | (nothing to measure) |
| Fonts (2 latin variable woff2, self-hosted, `font-display: swap`) | ≤ 72 KiB per file | Inter 47.1 KiB + Fraunces 65.7 KiB, immutable-cached, never block paint | same |
| Images on workflow pages | none | none | `templates_carry_no_images_or_csp_violations` |
| Third-party/external resources at runtime | none (Alpine is bundled, same-origin) | none | same |
| Asset caching | esbuild content hash in the URL + `public, max-age=31536000, immutable` | ✓ | `assets_serve_fingerprinted_with_an_immutable_cache_lifetime` |
| Compression | gzip (brotli/zstd also available) via `Compress` middleware | ✓ | `assets_compress_when_the_client_accepts_gzip` |

Consequences: a first page view costs the HTML plus one ~4.3 KiB
stylesheet and one ~21 KiB script (both immutably cached — every later
view costs the HTML alone). First meaningful content never waits on JS:
pages are server-rendered, navigation is real links, and every form works
before the bundle arrives (FRONTEND.md §7). No layout shift from late
chrome (notices are server-rendered; the shell reserves its regions), and
no blank-screen state (PRG keeps the previous page until the response
arrives; the submitting form shows a busy state via `aria-busy`).

## Backend benchmarks (Phase 8, run 2026-07-17 — `load/README.md` to reproduce)

**Shared metadata for all three classes.** Hardware: AMD Ryzen 9 3900X
(12c/24t), 31 GiB RAM, Linux 6.18.32-lts — generator (`wrk`), server, and
PostgreSQL 18.4 all on this one host over loopback (no network hop; real
deployments add one). Server: `cargo build --release` binary, default
config (64-connection pool). Dataset: 1 institution, 1 student account,
200 courses × 1 open section with capacity + 1 meeting each, growing to
~3.9k document requests during class C; database size 14 MB — a demo-scale
dataset, stated as such. Cache state: warm (each class ran once unmeasured
before the measured 30 s run; PostgreSQL default shared_buffers).
Durability: `synchronous_commit = on`, fsync on — **a raw single-row
`INSERT; COMMIT` in psql costs ~68 ms on this machine's storage**, which
is the floor for anything durable below. All authenticated runs share one
session (session resolution on every request; the idle-deadline slide
UPDATE writes at most once per 60 s per session — enforced in the
statement itself since the 2026-07-25 stampede fix below, not just by the
application-side staleness check). Error rate 0 in every measured run
(all responses 2xx/3xx, verified; class C row count matched the request
count).

| Class | Endpoint & mix | Concurrency | Throughput | p50 / p95* / p99 | Response size |
|---|---|---|---|---|---|
| A — in-process, no DB | `GET /health/live`, 100 % | 8 threads / 64 conns / 30 s | **635,941 req/s** | 80 µs / ~130 µs / 387 µs | 4 B body (~520 B with headers) |
| B — read path | `GET /api/v1/catalog?term_id&page=0` (auth; 4-table join + LATERAL aggregate, 20 rows/page), 100 % | 8 / 64 / 30 s | **3,940 req/s** | 15.4 ms / ~24.8 ms / 35.6 ms | 4,293 B JSON |
| C — durable transactional | `POST /ui/documents` (CSRF + availability check + request INSERT + audit INSERT, one committed tx per hit, unique idempotency key each), 100 % | 4 / 16 / 30 s | **106 req/s** | 133 ms / ~200 ms / 625 ms | 303 redirect (~580 B) |

\* wrk reports the 90th percentile; p95 shown as the 90th–99th bracket
midpoint marker — raw distributions in the wrk output reproduced by
`load/README.md`.

**Reading the numbers honestly.**
- Class B at 64 connections is saturation queueing, not per-request cost:
  a single warm catalog request completes in ~3.3 ms end to end.
- Class C is **fsync-bound by this workstation's storage** (68 ms per
  durable commit measured with no application at all); the application
  adds < 5 ms over the raw transaction. On server-grade storage with
  proper group commit this ceiling moves an order of magnitude; the
  number to hold the app to is the delta, not the absolute.
- First measured class-C run produced 2,683 responses but only 671 rows:
  wrk seeds every thread's RNG identically, all threads posted the same
  idempotency keys, and the server correctly returned the original
  request each time — an accidental proof that idempotency holds under
  concurrent load. The script now seeds per thread.

## Session-touch stampede fix + class B re-measurement (2026-07-25)

A benchmark run with a **stale** shared session (cookie minted minutes
before the run, unlike the fresh-cookie runs above) exposed a stampede:
all 64 in-flight requests read the same stale `last_seen_at`, all fired
the idle-slide UPDATE on the same `user_session` row, and each queued
writer paid its own ~68 ms durable commit to write the timestamp the
previous one had just written — a monotonic 1 s → 7 s lock queue, p99
474 ms, and 2 s client timeouts on a pure read path. Real users hold
distinct sessions, but any single high-concurrency API client reproduces
this.

Fix (`identity_access/sessions.rs`): the staleness threshold is repeated
inside the UPDATE (`AND last_seen_at <= now − 60 s`) so only the first
writer matches, and the row is selected `FOR NO KEY UPDATE SKIP LOCKED`
so the losing herd doesn't even wait for the winner's commit — measured
intermediate state showed the guard alone left a ~3 s convoy of no-op
waiters. Same pattern as the document-job claim query. Test:
`concurrent_refreshes_write_once_not_once_per_request`.

Re-measured class B after the fix, same hardware/dataset, quiet machine
(a leftover second server process from the interactive session had been
halving earlier numbers — generator noise, worth checking before trusting
any local run), stale-session start, warm caches:

| Concurrency | Throughput | p50 / p90 / p99 | Errors |
|---|---|---|---|
| 8 t / 64 conns | **7,633 req/s** | 7.4 ms / 12.4 ms / 423 ms | 35 timeouts / 229 k (0.015 %) |
| 4 t / 16 conns | **6,438 req/s** | 2.4 ms / 3.0 ms / 3.9 ms | 0 |

Zero slow-statement warnings in either run — the session row is no longer
a factor at any percentile. The c64 tail is saturation queueing (the box
tops out ~7.6 k req/s on this query; at c16 the p99 is 3.9 ms), consistent
with the "saturation, not per-request cost" note above.

## Realistic-scale benchmarks + plan inspection (2026-07-25, seed-demo dataset)

The 2026-07-17 numbers above ran against 200 uniform sections and one
student. Re-run against the `seed-demo` dataset — 900 students, 89
courses, 685 sections across four terms (278 open in the current term),
~14 k enrollments with real seat distribution, multi-term grade history —
same hardware, quiet machine, fixed binary, `VACUUM ANALYZE` after seed,
warm + measured. Class B/C authenticated as `demo.student`
(`term_id = …0020`, FALL-2026).

| Class | Configuration | Throughput | p50 / p90 / p99 | Notes |
|---|---|---|---|---|
| A | t8/c64 | 635,941 req/s | 79 µs / 138 µs / 424 µs | identical to both prior records |
| B | t8/c64 | 5,590 req/s | 10.8 / 17.3 / 25.7 ms | 0 errors |
| B | t4/c16 | 4,598 req/s | 3.4 / 4.1 / 5.3 ms | 0 errors |
| B, page 13 (OFFSET 260) | t4/c16 | 4,442 req/s | 3.5 / 4.3 / 5.5 ms | deep OFFSET costs ~3 % |
| B, `q=MATH` | t4/c16 | 14,605 req/s | 1.0 / 1.4 / 1.8 ms | filter shrinks the working set |
| C | t4/c16 | 95 req/s | 158 / 183 / 490 ms | 4,335 rows verified for ~4,314 requests |

Realistic scale costs class B ~27 % versus the tiny dataset (3.4 ms vs
2.4 ms p50 at c16): the pre-LIMIT working set is now 278 rows. Class C is
~20 % slower per request (transcript snapshots and availability checks
see real history) and remains fsync-bound.

**Plan inspection at this scale** (`EXPLAIN (ANALYZE, BUFFERS)`, stats
fresh):

- Catalog search: 1.5 ms. Seq scans on `section`/`section_capacity` are
  the planner's correct choice (695-row tables, 21 shared-buffer pages);
  the meeting LATERAL costs 0.002 ms × 278. The only wasted work in the
  plan: the LATERAL and sort run over all 278 open sections before LIMIT
  takes 20. Rewrite trigger: ~1,000+ open sections per term or a measured
  p50 regression — then wrap the ORDER BY/LIMIT in a subquery and join
  the meeting aggregate after it (keyset pagination is NOT indicated;
  OFFSET at this depth is ~3 %).
- Session resolve: 0.098 ms, index scans throughout
  (`user_session_token_hash_key` exists; the seq scan on a 2-row session
  table is size, not a missing index).
- Idempotency lookup: 0.043 ms via `document_request_idempotent`. (A
  test query with `gen_random_uuid()` in the WHERE showed a 3.8 ms seq
  scan — volatile functions forbid index conditions. Bind constants when
  hand-checking plans, as the app does.)
- Officer queue / student history / registrar sections: 0.8–2.4 ms even
  with 4.4 k benchmark-generated requests in the table. The cost of the
  known unpaginated pages (CURRENT_STATE.md findings) is HTML
  rendering/transfer, not the database.

**Verdict: no optimization earned.** Every hot path is an index scan or
a correct small-table seq scan; the only real pessimizations at scale
remain the unpaginated registrar/officer HTML pages already recorded as
UI findings.

## Generator-isolated class B re-measure (2026-07-29)

Question tested: was the colocated `wrk` stealing server CPU and
understating the class B ceiling? Method: same `ubedtech_bench` dataset
(post `VACUUM ANALYZE`), same binary, fresh session as `demo.student`,
warm + measured 30 s runs — first an unpinned control, then server
pinned to physical cores 0–7 (`taskset -acp 0-7,12-19`) and wrk pinned
to cores 8–11 (`taskset -c 8-11,20-23`; CPUs 12–23 are SMT siblings of
0–11 on this 3900X, so isolation is by whole core). PostgreSQL left
unpinned. Server process ran at nice 5 (background launch) — noted
because the box saturates during the run; postgres ran at nice 0.

| Run | Throughput | p50 / p90 / p99 | Errors |
|---|---|---|---|
| control, unpinned t8/c64 | 5,079 req/s | 12.0 / 18.9 / 27.0 ms | 0 |
| pinned t8/c64 | 5,038 req/s | 12.2 / 18.8 / 26.4 ms | 0 |
| pinned t4/c16 | 4,176 req/s | 3.7 / 4.7 / 6.1 ms | 0 |

**Answer: no.** Pinned and unpinned are statistically identical (−0.8 %),
and both match the 2026-07-25 realistic-scale record (5,590 / 4,598 —
within ambient variance). Generator interference is not a factor at
class B throughput; the hypothesis is closed.

Where the ceiling actually is, from a mid-run CPU sample (box 97.6 %
busy): the Rust server used ~1.75 cores; PostgreSQL backends — one per
pool connection, all runnable at 35–50 % each — consumed the bulk of the
remaining 24 threads executing the ~1.5 ms catalog query. Class B is
**PostgreSQL-CPU-bound**, exactly as the plan inspection predicted. The
levers, still in order and still unearned at UB scale: the recorded
LIMIT-first query rewrite (trigger: 1,000+ open sections), then more/
faster database cores. (Generator isolation would still matter for
class A, where the server itself can consume every core — untested
here.)

### PostgreSQL-side audit + pool-size A/B (same session)

Server-side cache state, checked not assumed: 100.00 % shared-buffer hit
ratio lifetime on `ubedtech_bench` (43 MB database inside the default
128 MB `shared_buffers`) — PostgreSQL's page cache is already fully
engaged, and PostgreSQL has no query-result cache to "enable". `work_mem`
(4 MB vs a 278-row sort), JIT (`jit_above_cost` 100k vs a ~40-cost
query), and parallel workers (LIMIT 20) are all non-factors at this
data size. No PostgreSQL configuration change is indicated.

Pool-size A/B at t8/c64 (64 runnable backends vs 24 hardware threads
looked like oversubscription — measured instead of assumed):

| `APP_DB_MAX_CONNECTIONS` | Throughput | p50 / p99 | Latency stdev |
|---|---|---|---|
| 64 (default) | 5,038 req/s | 12.2 / 26.4 ms | 4.7 ms |
| 20 | 4,622 req/s | 13.7 / **16.4 ms** | **0.9 ms** |

A tradeoff, not a pessimization: the big pool buys ~9 % throughput at
saturation; the small pool collapses the p99 by 40 % because requests
queue briefly at the pool instead of context-switching inside PostgreSQL.
At realistic load (~200 req/s) neither setting is ever felt. Default
stays 64; the knob already exists for a latency-sensitive deployment.

## Query plans (Phase 8 inspection, hot enrollment + document paths)

`EXPLAIN ANALYZE` on the load dataset (caveat: 200 sections / ~3.9k
requests — planner choices at real scale may differ, index paths exist):

- Enrollment idempotency lookup, seat-reservation UPDATE, document job
  claim (`SKIP LOCKED`), student request list, download-authorization
  join, officer queue: **all index scans, all < 0.2 ms execution**
  (`document_request_admin_queue`, `_student_history`, `_idempotent`,
  `document_job_claim` partial index, `generated_document_current`,
  enrollment unique/partial indexes).
- **Found and fixed:** `section_meeting` had no index on `section_id`, so
  the catalog's per-section LATERAL meeting aggregate seq-scanned it once
  per row (and the schedule view does the same lookup). Migration 0017
  adds `section_meeting_by_section (section_id, day_of_week, starts_at)`.
  At 200 rows the planner still prefers the seq scan (correctly); the
  index is for real-scale data.
- Catalog pagination is LIMIT/OFFSET over an ordered join — fine at the
  20-row page size and current scale; keyset pagination is the upgrade if
  a profile ever shows deep-offset pages in the wild.
