# Benchmark compilation — 2026-07-25 interactive session

Every run from the 2026-07-25 benchmarking session, including the broken
ones (labeled — a broken run that looks plausible is how wrong numbers
get quoted later). `docs/PERFORMANCE.md` remains the canonical
performance record; this file is the dated raw compilation behind its
2026-07-25 update. Reproduction steps: `load/README.md`.

## Shared metadata

- Hardware: AMD Ryzen 9 3900X (12c/24t), 31 GiB RAM, Linux 6.18.32-lts.
- Generator (`wrk`), server, and PostgreSQL 18.4 colocated on this host,
  loopback (no network hop, no TLS — real deployments add both).
- Server: release binary, default config (64-connection DB pool),
  `synchronous_commit = on`. Raw single-row `INSERT; COMMIT` in psql on
  this storage: ~68 ms — the floor for anything durable.
- Dataset: `src/dev/seed.sql` + `load/seed.sql` (1 institution, 200
  courses × 1 section, 1 student), fresh `ubedtech_load` database.
- All authenticated runs share ONE session — the worst case for the
  session-touch path, and deliberately so (see the stampede below).
- Binary versions: "pre-fix" = before commit `462bbc0`
  (session idle-slide stampede fix), "fixed" = at or after it.
- Machine quiet unless noted. Two background-noise incidents are called
  out — both halved throughput without changing the shape of anything.

## Class A — in-process gate (`GET /health/live`, no DB)

| Run | Conditions | Throughput | p50 / p90 / p99 | Errors |
|---|---|---|---|---|
| warm | pre-fix, t8/c64 | 640,083 req/s | 79 µs / 132 µs / 301 µs | 0 |
| measured | fixed, t8/c64, warm+measured pair | 631,894 → 635,922 req/s | 79 µs / 136 µs / 288 µs | 0 |
| **c1000** | fixed, t24/c1000, `ulimit -n 65536` both sides | **730,020 req/s** | 721 µs / 5.2 ms / 14.2 ms | 0 |
| c1000, default ulimit | **INVALID** — server hit EMFILE (`Too many open files`), accept loop refused most connections | — | — | — |

Findings: matches the 2026-07-17 baseline (635,941). c1000 is *faster*
than c64 — at c64 the colocated generator and server contend for the
same 24 threads and the pipeline is under-fed; deeper connection queues
feed it better. The default `ulimit -n 1024` caps the server at ~950
concurrent connections; production needs `LimitNOFILE=` (or equivalent)
raised.

## Class B — authenticated read path (`GET /api/v1/catalog`, 4-table join + LATERAL, 20 rows)

| Run | Conditions | Throughput | p50 / p90 / p99 | Errors |
|---|---|---|---|---|
| stampede | pre-fix, t8/c64, **stale session at start** | 3,523 req/s | 16.3 / 26.9 / **474 ms** | **32 timeouts** |
| guard-only | intermediate fix (WHERE guard, no SKIP LOCKED), t8/c64, stale start, noisy box | 3,652 req/s | 16.0 / 26.6 / 663 ms | 20 timeouts |
| fixed | + SKIP LOCKED, t8/c64, stale start, noisy box | 3,578 req/s | 15.9 / 26.6 / 390 ms | 34 timeouts |
| fixed, quiet | same, second server killed + vacuum | **7,633 req/s** | 7.4 / 12.4 / 423 ms | 35 timeouts (0.015 %) |
| fixed, quiet, c16 | t4/c16 | 6,438 req/s | 2.4 / 3.0 / **3.9 ms** | 0 |
| user re-run, c64 | fixed, stale start, warm+measured; ambient load ~halves absolutes | 3,652 → 3,659 req/s | 16.6 / 26.2 / **36.6 ms** | 0 |
| user re-run, c16 | same box | 3,019 → 2,994 req/s | 5.3 / 6.4 / 7.9 ms | 0 |

Findings, in order of discovery:

1. **Session-touch stampede (pre-fix)**: 64 in-flight requests on one
   stale session all fired the idle-slide UPDATE on the same row; each
   queued writer paid its own ~68 ms fsync to write the timestamp the
   previous one just wrote (log showed a 1 s → 7 s monotonic lock queue).
2. **Guard alone was not enough**: with the staleness threshold repeated
   in the WHERE, queued statements no-oped (`rows_affected: 0`) — the
   redundant fsyncs were gone — but the herd still convoyed ~3 s
   *waiting* on the row lock before discovering it had nothing to do.
3. **SKIP LOCKED closed it**: losers skip instead of waiting (the
   job-claim pattern). Zero slow-statement warnings in every run since,
   including runs deliberately started on a stale session.
4. The residual c64 tail (~400 ms p99 on the quiet box) is saturation
   queueing at the machine's ~7.6 k req/s ceiling for this query — at
   c16 the p99 is 3.9 ms flat. Per-request cost and saturation behavior
   are different numbers; report both.
5. Background processes are a silent 2× error: a leftover second server
   halved every reading until it was found. Check `pgrep` before
   trusting any local number.

## Class C — durable transactional path (`POST /ui/documents`, INSERT + audit INSERT, one committed tx per hit)

| Run | Conditions | Throughput | p50 / p90 / p99 | Write check |
|---|---|---|---|---|
| no CSRF env | **INVALID** — lua script failed to load, wrk sent bare `GET /`: 274,825 req/s of 100 % non-2xx, zero rows written | — | — | 0 new rows |
| documented, pre-fix | t4/c16 | 129 req/s | 117 / 144 / 269 ms | 3,898 rows = 3,881 req + 17 seed ✓ |
| c64 probe, pre-fix | t8/c64 (improvised saturation probe, not the documented class) | 365 req/s | 133 / 270 / 1,500 ms | 47 timeouts; stampede at start + ~1.67 s COMMIT bursts |
| documented, fixed | t4/c16 (user re-run) | 117 req/s | 133 / 142 / 233 ms | 3,537 rows = 3,520 req + 17 seed ✓ |

Findings: fsync-bound by design and by measurement — throughput is the
disk's group-commit rate (16 writers ≈ 117–129 req/s; 64 writers ≈ 365
req/s with a collapsed tail past the knee). The app adds < 5 ms over a
raw transaction. The 2026-07-17 baseline (106 req/s / p99 625 ms) is
consistent. Always verify the row count after a class C run: the two
invalid failure modes seen so far (shared RNG seeds in 2026-07-17's
first run, missing CSRF here) both produce plausible-looking wrk output
and zero real writes.

## Prior baseline for reference (2026-07-17, PERFORMANCE.md)

A: 635,941 req/s · B: 3,940 req/s, p99 35.6 ms · C: 106 req/s, p99 625 ms.

## Operational findings this session

- `DATABASE_URL` needs an explicit username — sqlx falls back to a
  default role, unlike psql (fixed in `load/README.md`, `80c7486`).
- Default `ulimit -n 1024` caps concurrent connections at ~950;
  raise `LimitNOFILE` for production units.
- Session idle-slide stampede fixed in `462bbc0`; test
  `concurrent_refreshes_write_once_not_once_per_request` pins it.
