# Building a System That Lasts: A First-Principles Tutorial

*A worked example, using this project's real code and real measurements.
Every number in this document was measured on this codebase; nothing is
hypothetical. Read it to understand this system — but also to learn the
method, which transfers to any system you build next.*

---

## How to read this

Software design taught from patterns is like math taught from formulas:
you can pass the test and still not know what to do with a new problem.
This document teaches by derivation instead. Each section states the
problem, shows the reasoning steps, and checks the answer against a
measurement — like a worked math problem. The sequence matters:

```
IDEA  →  CODE  →  CONSTRAINTS  →  INFRASTRUCTURE
(what)   (how)    (what must     (only what the
                   never break)    measurements demand)
```

and inside the performance work, a strict ladder:

```
1. Right algorithm      (the biggest wins live here)
2. Non-pessimization    (remove work that didn't need to happen)
3. Optimization         (measured experiments; most get REVERTED)
4. Infrastructure       (hardware, topology — only after 1–3 are proven done)
```

Climbing this ladder out of order is the root cause of most slow,
complicated systems: teams reach for caches (step 3) and clusters
(step 4) to compensate for a bad query (step 1) — and then maintain the
cache, the cluster, *and* the bad query forever.

---

## Part I — The Idea

**Problem statement.** A university needs course registration, grades,
transcripts, and official documents. Peak load is registration morning:
a few thousand students hitting the catalog and registering.

**Step 1: put honest numbers on it before writing any code.**

```
students:                  ~7,000
classes per student:       ~5
registration burst window: minutes, not seconds
   ⇒ writes:  7,000 × 5 = 35,000 registrations, spread over minutes
   ⇒ ~100–300 writes/second at the worst spike
   ⇒ reads (catalog browsing): maybe 10–50× that, ~200 req/s sustained
```

This arithmetic is the most important design act in the project. Every
later decision is judged against ~200 req/s, not against an imagined
million. Systems implode when they're built for the imagined number: the
imagined number justifies microservices, message queues, and caching
layers, and each of those is a permanent tax paid against traffic that
never arrives.

**Step 2: identify what is genuinely hard.** Not the load — the
*invariants*:

- A section with 30 seats must never hold 31 students, even when two
  students race for the last seat in the same millisecond.
- A grade, a registration, an official document must never be reported
  as saved unless it is durably on disk. A false "Enrolled" is worse
  than an honest error.
- A revoked session must be dead immediately, on every path.

The hard part of this system is correctness under concurrency and
crashes, not throughput. That realization decides the architecture.

---

## Part II — The Code

**Step 1: choose the simplest shape that can possibly work.**

One Rust binary. One PostgreSQL database. Feature areas are directories
and modules with typed interfaces — not services, not packages:

```
backend/src/
  identity_access/   academics/   enrollment/   records/
  documents/         licensing/   institution/  jobs/
  shared/  audit.rs  app.rs  config.rs  db.rs  main.rs
```

Why is this the *first-principles* answer and not just the lazy one?
Derive it:

```
A module boundary costs:        a function call        (~nanoseconds)
A process boundary costs:       serialization + a network hop
                                + a failure mode + a deploy unit
                                + a version-skew problem   (~forever)

Both give you the same thing:   separation of concerns.
   ⇒ Pay for the boundary you need (separation),
     not the one you don't (distribution).
```

Microservices solve an *organizational* scaling problem (hundreds of
engineers shipping independently). This project has a handful. The
module boundary delivers all the separation at none of the distributed
cost — and Part IV shows the measurement that proves how brutal that
cost is (spoiler: −96 %).

**Step 2: the handmade dependency rule.** Every dependency is code you
now own but didn't write and can't fully see. The rule used here:

> A dependency earns its place only when it encodes knowledge you should
> not reinvent (cryptography, TLS, an async runtime, SQL drivers).
> Everything else — session tokens, CSRF, pagination, job queues,
> caching — is a few dozen lines against the standard library and the
> database you already run.

Concretely: sessions are opaque random tokens, SHA-256-hashed into a
PostgreSQL table — ~100 lines, zero dependencies, fully inspectable,
trivially revocable. The fashionable alternative (JWT libraries, token
refresh flows, key rotation machinery) was rejected in the design docs
because *statefulness was never actually a problem*: the app has exactly
one database it always talks to anyway. The background job system is a
PostgreSQL table with `FOR UPDATE SKIP LOCKED` — not a message broker.
The reaper for crashed workers is one `UPDATE` on a timestamp column —
not a heartbeat protocol.

This is the core of "built to last": in five years, a new engineer can
read every line that authenticates a user or claims a job. Nothing can
rot in a dependency you don't control, because the load-bearing parts
have almost none.

**Step 3: put every invariant where it cannot be bypassed.** The
application checks rules to produce good error messages; the *database
constraint* is the actual guarantee:

- `enrolled_count <= capacity` is a CHECK constraint, not a code path.
- "every section has a capacity row" is a trigger created in the same
  migration as the table (a prior review found the missing-capacity-row
  case produced the same error as "section full" — fixed by making the
  invalid state unrepresentable, then testing that it fails *loudly and
  distinctly*).
- Audit records are written in the SAME transaction as the change they
  describe. Not after. A crash between "change" and "audit" cannot
  produce an unaudited change, because there is no between.

---

## Part III — The Constraints (correctness before speed)

**The last-seat problem, worked in full.** Two students, one seat,
same instant. The naive algorithm:

```
1. SELECT enrolled_count, capacity FROM section_capacity ...   -- read: 29 < 30
2. (both requests pass the check simultaneously)
3. UPDATE ... SET enrolled_count = 30                          -- both write
   ⇒ 31 students in 30 seats. Oversold.
```

The read-check-write shape is *algorithmically* wrong under
concurrency — no amount of locking advice, retries, or infrastructure
fixes a wrong algorithm cleanly. The right algorithm makes the check
and the write one atomic statement:

```sql
UPDATE section_capacity
   SET enrolled_count = enrolled_count + 1
 WHERE section_id = $1
   AND enrolled_count < capacity        -- the check IS in the write
RETURNING enrolled_count;
```

Zero rows back = full section (or a claimed registrar override — an
explicit record of who, why, and which enrollment consumed it). One row
back = the seat is yours, and PostgreSQL's row lock made every
concurrent competitor wait and re-evaluate against the *new* count.
The proof is a test that races two registrations for the last seat and
asserts exactly one success — that test is the architecture's load-
bearing wall, and it stays green in every phase.

**Durability as a floor, not a feature.** Every truthful operation
(register, grade, document) is one transaction ending in one durable
commit. Measured on this workstation's SSD:

```
raw single-row INSERT; COMMIT in psql:      60–94 ms   (that's the fsync)
full registration through the whole stack:  57–63 ms
   ⇒ the application's overhead over the physics: ~zero
```

Sit with that: session resolution, CSRF, seven policy checks
(idempotency ×2, window, holds, duplicate, completed-course,
prerequisites, schedule-conflict), the seat reservation, the enrollment
insert, the audit row, the commit, and rendering the response — all of
it fits *inside the measurement noise of a single disk sync*. That is
what "non-pessimized" means: not "fast tricks", but *the code costs
nothing beyond what the problem irreducibly costs.*

**Crash-safety by idempotency.** Every registration carries a client
idempotency key. A crash after commit but before the response reaches
the browser → the user retries → the service returns the *original*
receipt instead of double-registering. The benchmark accidentally
proved this: an early load-test seeded every thread's RNG identically,
all threads posted the same keys, and the server correctly collapsed
2,683 requests into 671 rows. The "bug" in the load script was a free
integrity test of the system.

---

## Part IV — The Performance Ladder, As It Actually Happened

The catalog endpoint (the hottest read: 4-table join + per-section
meeting aggregate + seats + pagination) went from 3,940 → 13,951 req/s
with **zero** infrastructure. Here is the ladder, step by step, with
the arithmetic.

### Step 0: Baseline honestly, or every later number is fiction

Rules used throughout: three benchmark classes never conflated
(in-process / read path / durable write), quiet machine verified (a
leftover server process once halved the numbers; a squatting connection
pool once blocked startup entirely — both found *because* the protocol
checks), warm caches declared, dataset size stated, p50/p90/p99 + error
rate + transfer always recorded, and every result written into
`PERFORMANCE.md` with enough context to be reproduced or distrusted.

```
Baseline (t8/c64):  3,940 req/s,  p50 15.4 ms
```

### Step 1: Right algorithm — twice

**1a. The session-touch stampede.** A benchmark run with a stale shared
session exposed it: all 64 in-flight requests read the same stale
`last_seen_at`, all fired the idle-slide UPDATE on the same row, and
each queued writer paid its own ~68 ms durable commit to write a
timestamp the previous writer had just written. p99: 474 ms. On a pure
read path.

The fix is algorithmic, not infrastructural — make only the first
writer match, and make the losers not even wait:

```sql
UPDATE user_session SET last_seen_at = $2, idle_expires_at = $3
 WHERE id IN (SELECT id FROM user_session
               WHERE id = $1 AND last_seen_at <= $4     -- staleness guard
                 FOR NO KEY UPDATE SKIP LOCKED)          -- losers skip
```

```
Result: 3,940 → 7,633 req/s (+94 %)
```

**1b. The LIMIT-that-came-too-late.** Plan inspection (`EXPLAIN
ANALYZE`, not intuition) showed the catalog query computing the meeting
aggregate and sort for *all* ~280 open sections, then keeping 20:

```
work done:    280 sections × (LATERAL aggregate + wide row)
work needed:   20 sections × (LATERAL aggregate + wide row)
                + one skinny sort of 280 (code, section_code, id) rows
```

Rewrite: paginate over skinny ids first, join the expensive parts onto
only the 20 winners.

```
query alone:   2.94 ms → 0.79 ms            (3.7×)
end to end:    7,633  → 13,951 req/s        (+83 %)
p99 at c64:    423 ms → 8.9 ms              (47×)
```

Note what the p99 collapse teaches: cheaper queries mean fewer runnable
database backends fighting for cores, so the *tail* — which is pure
queueing — vanishes with the waste. Latency tails are usually a queue
symptom, not a code symptom.

### Step 2: Non-pessimization — count what actually happens

Instrument, don't assume. Transactions per catalog request, measured
via `pg_stat_database` counters: **2.2** (one session resolve, one
catalog query, a sliver of background polling). A recorded plan to
"fold the session touch into the resolve query" turned out to be
already moot — the stampede fix made the touch fire once per 60 s, not
per request. The lever was struck from the docs *with the measurement
attached*, so nobody re-chases it.

Then the registration write path got the same census (Part III's
57–63 ms result): **zero pessimization found, zero changes made.** That
outcome — *changing nothing, on evidence* — is a first-class result.
Optimization without a measured target is how codebases accrete clever
unreadable code that saves nothing.

### Step 3: Optimization experiments — and the discipline of reverting

**The cache that won and was still rejected.** An in-process cache
(moka) on session resolution: pre-registered prediction of the ceiling
(~8 %, the session lookup's share of the request), built, measured:

```
predicted:  ≤ 8 %        measured:  +7.8–8.2 %     (exactly at ceiling)
```

The win was real. It was reverted anyway, because the cost was
permanent and asymmetric: every present *and future* revocation path
must remember to invalidate, and forgetting **fails open** — a
suspended user keeps an authenticated session for the TTL. Six
existing revocation-immediacy tests caught this on contact (tests as
tripwires, not ceremony). A follow-up experiment measured the
multi-instance version: logout through instance A leaves instance B
serving the dead session for the full 5.000 s TTL. In-process session
caching is *structurally* disqualified for any horizontally scaled
deployment — a finding that outlives both experiments.

The verdict lives in an ADR with an explicit **adopt trigger** (>1
instance AND session resolution measured as the bottleneck). This is
how you say "no" without losing the work: the branch keeps the
implementation, the doc keeps the numbers, and the trigger keeps the
decision honest when conditions change.

**Knowing where the ceiling is.** With the app at 13.8k req/s, what's
left? Measure the layers separately:

```
SELECT 1 through the wire, 24 clients:        604,000 stmts/s
the real catalog query alone (pgbench):        23,261 req/s @ 1.03 ms
the full app:                                  13,951 req/s
```

Three numbers, three conclusions: protocol overhead is a non-factor
(we use 69k of a 604k budget); the database-side ceiling for this
workload is ~23k; the app sits at 59 % of it, and the gap is the
security-mandatory session resolve plus the app sharing the same 12
cores as PostgreSQL. No code change closes that gap honestly — which
is precisely the exit condition for Step 3 and the *entry* condition
for Step 4. You are only allowed to buy hardware once you can write
this table.

### Step 4: Infrastructure — sized by arithmetic, not vibes

**Pool size.** 24 hardware threads. Measured A/B at three sizes:

```
pool 15:  2,940 req/s   p99 24.6 ms   (starved: 64 clients / 15 slots)
pool 64: 13,951 req/s   p99  8.9 ms   ← default
pool 96: 13,834 req/s   p99 11.6 ms   (wash; oversubscription grows)
```

Bigger pools past ~2–3× the core count buy nothing and cost real
headroom: 96 app connections under PostgreSQL's default
`max_connections = 100` left 4 slots for everything else — and this
project *lived* that failure mode when an unrelated app's pool
exhausted the shared server and ours couldn't start. Pools are a
concurrency valve, not a performance dial.

**Overload shape.** 1,000 concurrent connections against the pool of 64:

```
throughput: 13,795 req/s  (−1 %, i.e. unchanged)
p50:        72.0 ms   — exactly the closed-loop prediction:
                         1000 in-flight ÷ 13.8k/s ≈ 72 ms
p99:        77.5 ms   — 5.5 ms above p50. A queue, not a cliff.
errors:     0 in 415,199 requests
```

A correctly shaped system degrades into a *fair line*, not a failure.
If your p50 under overload doesn't match `concurrency ÷ throughput`,
something is thrashing.

**The topology measurement that settles an argument forever.** Move
PostgreSQL to a managed cloud instance (53 ms away), keep everything
else identical:

```
each request = 4–6 sequential round trips
             ⇒ 4–6 × 53 ms ≈ 215–325 ms per request
             ⇒ closed-loop: 64 ÷ 0.325 s ≈ 197 req/s predicted
measured:                                   194 req/s   (−96 %)
```

Both machines nearly idle. The wire ate everything. Standing
conclusion: the app and its database belong on the same host or
sub-millisecond network, *always*, and if a hop ever becomes mandatory
the first lever is cutting round trips — not upgrading either machine.

**Writes are priced by physics.** The durable path:

```
one fsync on this SSD:            ~68 ms  ⇒ ~15 sequential commits/s
16 concurrent students measured:  117.8 committed registrations/s
                                  ⇒ group commit batches ~8 tx/fsync
verification: 3,536 requests = 3,536 audit rows. Zero fake successes.
```

~118 real registrations/second on a workstation = all 35,000
registration-day writes in five minutes. And the only remaining lever
is hardware: server-grade storage moves the fsync floor an order of
magnitude, with zero code changes. That is what it looks like when the
software is *finished*: the performance roadmap reduces to a shopping
list.

### The ladder, scored

```
Step 1 (algorithm):        3,940 → 13,951 req/s   and p99 423 → 8.9 ms
Step 2 (non-pessimization): confirmed nothing left; two levers struck
Step 3 (optimization):      +8 % available — rejected on security cost
Step 4 (infrastructure):    pool sized by math; scale-out priced (−96 %
                            if done naively); storage identified as the
                            one honest lever left
```

The entire 3.5× win came from steps 1–2: reading plans, counting
statements, deleting waste. Steps 3–4 mostly produced *documented
refusals* — and those refusals, with their numbers and triggers, are as
valuable as the wins. They are why the system will still be simple in
five years.

---

## Part V — Future Changes (how to keep it alive)

The system carries its own upgrade path as recorded triggers, so
future maintainers act on conditions, not archaeology:

| Trigger (measured condition) | Prepared response |
|---|---|
| 1,000+ open sections per term, or catalog p50 regresses | Already done (the LIMIT-first rewrite); next: keyset pagination only if deep OFFSET shows up in profiles |
| >1 app instance AND session resolution measured as bottleneck | Shared-store session cache from the experiment branch (delete-on-revoke, fall-through-on-unreachable), re-priced against real network latency |
| Write throughput matters beyond ~120/s | Server-grade NVMe / proper group-commit storage: ~10× for zero code |
| Database CPU saturates on reads | More cores — it scales nearly linearly; the query work is already minimal |
| A network hop between app and DB becomes mandatory | Cut round trips first (fold session touch into resolve); measured math says the wire dominates everything else |

And the maintenance constitution that keeps entropy out: every slice
lands with its tests green and a commit that names what became true;
every deviation from the design gets a numbered ADR; every benchmark
gets its context recorded; every abandoned experiment keeps its branch,
its numbers, and its re-entry trigger. Documentation here is not
description — it is *stored decisions with their evidence*, which is
the only documentation that stays true.

---

## Part VI — The Transferable Method (the whole tutorial in 12 lines)

```
 1. Put real numbers on the load before designing.       (Part I)
 2. Find the invariants; they are the actual problem.    (Part I)
 3. Choose the simplest topology that separates concerns:
    modules first, processes only when organizationally forced. (II)
 4. Own your load-bearing code; depend only for encoded expertise. (II)
 5. Make invalid states unrepresentable (constraints, triggers,
    same-transaction audit).                              (II, III)
 6. Make racy checks atomic writes; prove it with a race test. (III)
 7. Measure the physics floor (fsync, RTT); build against it. (III, IV)
 8. Optimize in order: algorithm → waste-removal → experiments →
    hardware. Never skip ahead.                           (IV)
 9. Pre-register predictions; measure; REVERT wins whose costs are
    permanent. Record the adopt trigger.                  (IV.3)
10. Know your ceiling per layer; only buy hardware once the gap to
    the ceiling is security or physics.                   (IV.3–4)
11. Under overload, verify latency = concurrency ÷ throughput;
    anything worse is thrash.                             (IV.4)
12. Ship every conclusion with the number that proves it, in a doc
    the next person will actually find.                   (V)
```

---

## Part VII — Books That Deepen Each Part

Curated, not exhaustive — each mapped to the part of this tutorial it
extends.

**The mindset (Parts I–II: simplicity, longevity, owning your code)**

- **"A Philosophy of Software Design" — John Ousterhout.** The best
  modern text on why deep modules with small interfaces beat shallow
  layers, and why complexity is incremental debt. This project's
  module-not-microservice choice is Ousterhout applied.
- **"The Practice of Programming" — Kernighan & Pike.** Old, short,
  permanently right: simplicity, clarity, generality, and choosing the
  boring algorithm that is correct on edge cases.
- **"The Pragmatic Programmer" (20th Anniversary) — Hunt & Thomas.**
  Orthogonality, tracer bullets, and the discipline of small reversible
  steps — the commit-per-slice rhythm used here.
- **"Software Engineering at Google" — Winters, Manshreck, Wright
  (eds.).** Read it for the parts about time: "software engineering is
  programming integrated over time." The dependency chapter is the
  strongest published argument for the handmade rule in Part II.

**Correctness under concurrency and failure (Part III)**

- **"Designing Data-Intensive Applications" — Martin Kleppmann.** The
  single most valuable book on this list. Transactions, isolation
  levels, replication, and exactly the read-check-write races worked
  in Part III — with the same "derive it, don't memorize it" style.
- **"Transaction Processing: Concepts and Techniques" — Jim Gray &
  Andreas Reuter.** The 1,000-page classic behind every claim about
  durability, group commit, and locking in this document. Skim-read it
  once; consult it forever.
- **"Release It!" (2nd ed.) — Michael Nygard.** Failure modes of
  production systems: pools, timeouts, backpressure, and why overload
  must become a queue and not a cliff (Part IV.4's shape, as war
  stories).

**Performance and non-pessimization (Part IV)**

- **"Systems Performance" (2nd ed.) — Brendan Gregg.** The measurement
  discipline: USE method, understanding whether you are CPU-, I/O-, or
  queue-bound before touching anything. Part IV's layer-by-layer
  ceiling table is this book's method in miniature.
- **"Computer Systems: A Programmer's Perspective" — Bryant &
  O'Hallaron.** Why the memory hierarchy, syscalls, and I/O cost what
  they cost. The intuition that a loopback round trip is ~0.05 ms but
  an fsync is ~10,000× that comes from this level of understanding.
- **"Data-Oriented Design" — Richard Fabian.** The handmade-adjacent
  argument that performance comes from shaping data for the machine —
  the same instinct as the skinny-rows-first catalog rewrite, applied
  to memory instead of SQL.

**Databases specifically (Parts III–IV, as requested)**

- **"SQL Performance Explained" — Markus Winand.** Short, surgical,
  and the best return-per-page on this list: indexes, join order, and
  why `LIMIT` placement decides how much work a query does — literally
  the Part IV.1b rewrite, generalized. (His site, use-the-index-luke
  .com, is the free companion.)
- **"PostgreSQL Internals" ("PostgreSQL 14 Internals") — Egor Rogov.**
  Free PDF from PostgresPro. Buffers, WAL, locks, vacuum, and the
  planner — everything `EXPLAIN (ANALYZE, BUFFERS)` assumes you know.
- **"Database Internals" — Alex Petrov.** How storage engines and
  distributed databases actually work under the SQL: B-trees, LSM
  trees, replication consensus. Pairs with Kleppmann — Petrov goes
  down the stack where Kleppmann goes across it.
- **"The Art of PostgreSQL" — Dimitri Fontaine.** The application
  developer's view: pushing logic into SQL where it belongs (the
  atomic seat reservation is this book's philosophy in one statement).

**Reading order if you're starting from zero:** Ousterhout →
Kleppmann → Winand → Gregg, then the rest as the problems arrive. Four
books, and you can re-derive most of this tutorial yourself — which
was the point.

---

*Everything above is checkable: the benchmarks live in
`docs/PERFORMANCE.md` with their full context, the experiments keep
their branches (`exp/moka-session-cache`, `exp/redis-session-cache`),
the race test runs in every CI pass, and the load scripts in
`backend/load/` reproduce every number on your own hardware. Distrust
any document like this one that doesn't end with a sentence like that.*
