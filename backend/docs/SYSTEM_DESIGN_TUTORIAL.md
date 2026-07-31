# The UB Education Platform: Full System Guideline & First-Principles Tutorial

*This is the front door to the whole application. It explains what the
system is, why every major decision was made, how it was measured, and
how to change it without breaking it. It is written as a worked
example — every number was measured on this codebase, every claim is
backed by a test, a benchmark, or an ADR you can open. Deep-dives live
in the sibling documents; this file is the map and the reasoning.*

**The document index (what lives where):**

| Document | What it holds |
|---|---|
| `../../CLAUDE.md` | The engineering constitution: architecture rules, security baseline, testing discipline, prohibitions |
| `../../FRONTEND.md` | The frontend constitution: motion rules, truthful-UI rules, fragment-vs-navigation line |
| `../../README.md`, `../../RUN.md` | Build, run, seed, release — local dev and release-package paths |
| `PERFORMANCE.md` | Every benchmark with full context; the source for all numbers here |
| `SECURITY.md` | The threat review, item by item, each claim test-backed or grep-verified |
| `PERMISSIONS.md` | Role × operation matrix — a row exists only if a passing test backs it |
| `ARCHITECTURE_DECISIONS.md` | Numbered ADRs: every deviation, with evidence and consequences |
| `API.md` | The HTTP surface: routes, auth requirements, error contract |
| `TESTING.md` | The four gates, database-test mechanics, the pipe-eats-exit-code trap |
| `OPERATIONS.md` | Config reference, deployment topology, worker/reaper behavior |
| `BACKUP_AND_RESTORE.md` | What to back up and the restore drill |
| `../load/README.md` | Reproducing every benchmark class on your own hardware |

---

## How to read this

Software design taught from patterns is like math taught from formulas:
you can pass the test and still not know what to do with a new problem.
This document teaches by derivation. Each section states the problem,
shows the reasoning, and checks the answer against a measurement. The
sequence is the method:

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

Climbing out of order is the root cause of most slow, complicated
systems: teams reach for caches (step 3) and clusters (step 4) to
compensate for a bad query (step 1) — then maintain the cache, the
cluster, *and* the bad query forever.

---

## Part 0 — What This System Is

One Rust/Actix binary. One PostgreSQL database. Server-rendered HTML
with minimal JavaScript. It runs a university: catalog and
registration, grades and transcripts, official documents (rendered by
a background worker inside the same binary), institution
administration, licensing, and a full audit trail.

**The module map** (directories under `backend/src/`, each a feature
area with private internals and a typed service API — the boundaries
are modules, not processes):

```
identity_access/  who you are: accounts, sessions, CSRF, throttling, roles
institution/      the tenant: terms, calendar/events, settings, admin
academics/        what is taught: courses, sections, meetings, catalog
enrollment/       who sits where: registration, drops, overrides, capacity
records/          what was earned: grades, GPA, transcripts, history
documents/        what is certified: requests, approval queue, PDF worker
licensing/        whether the deployment may serve: license state, import
jobs/             the background worker loop + stale-job reaper
shared/           Actor, error type, asset fingerprints — small by design
audit.rs          the same-transaction audit writer every feature uses
```

**Request lifecycle** (every authenticated request walks this path):

```
cookie → session middleware (opaque token, SHA-256 lookup,
         deadline + version + status checks, throttled idle-touch)
       → CSRF middleware (every browser mutation; X-CSRF-Token or form field)
       → license gate (402 on locked institutions, exempt: login/health/licensing)
       → handler (Actix types stop here)
       → service (plain Rust: policy checks, ONE transaction, audit row inside it)
       → template/fragment or JSON  (errors: {"code","message"}, never SQL)
```

**Roles** (each cell of capability is test-backed in `PERMISSIONS.md`):
student, instructor, registrar, records_officer, document_officer,
institution_admin, platform_licensing_admin. Authorization lives in
services — button visibility is UI, never enforcement.

---

## Part I — The Idea

**Step 1: put honest numbers on the load before designing.**

```
students:                  ~7,000
classes per student:       ~5
registration burst window: minutes, not seconds
   ⇒ writes:  35,000 registrations spread over minutes ≈ 100–300/s spike
   ⇒ reads:   catalog browsing at maybe 10–50× that ≈ 200 req/s sustained
```

This arithmetic is the most important design act in the project. Every
later decision is judged against ~200 req/s, not an imagined million.
Systems implode when built for the imagined number: it justifies
microservices, brokers, and cache layers, each a permanent tax paid
against traffic that never arrives.

**Step 2: identify what is genuinely hard.** Not the load — the
invariants:

- 30 seats must never hold 31 students, even when two students race
  for the last seat in the same millisecond.
- A registration, grade, or official document must never be reported
  saved unless it is durably on disk. A false "Enrolled" is worse than
  an honest error.
- A revoked session must be dead immediately, on every path.
- An institution's admin's power ends at their institution's boundary,
  in every query *and* every unique constraint.

The hard problem is correctness under concurrency and crashes. That
realization — not throughput — decides the architecture.

---

## Part II — The Code

**Step 1: the simplest shape that can possibly work.** Derive the
topology from the cost of boundaries:

```
A module boundary costs:   a function call                  (~ns)
A process boundary costs:  serialization + a network hop
                           + a failure mode + a deploy unit
                           + version skew                    (~forever)
Both buy the same thing:   separation of concerns.
   ⇒ Pay for the boundary you need, not the one you don't.
```

Microservices solve an *organizational* problem (hundreds of engineers
shipping independently) that this project does not have. Part IV
carries the measurement that prices the distributed alternative:
putting just the *database* a network hop away cost −96 %.

**Step 2: the handmade dependency rule.** Every dependency is code you
own but didn't write and can't fully see. The rule:

> A dependency earns its place only when it encodes expertise you
> should not reinvent: cryptography (argon2, ed25519), TLS, the async
> runtime, the SQL driver, the template engine. Everything else —
> sessions, CSRF, job queues, pagination, caching, PDF layout — is a
> few dozen lines against the standard library and the database you
> already run.

Applied here: sessions are opaque random tokens hashed into a table
(~100 lines, trivially revocable, fully inspectable — no JWT machinery,
because statelessness was never a requirement: there is exactly one
database and the app always talks to it). The job queue is a table
claimed with `FOR UPDATE SKIP LOCKED` — not a broker. The crashed-
worker reaper is one `UPDATE` on `locked_at` — not a heartbeat
protocol. In five years a new engineer can read every line that
authenticates a user or claims a job; nothing load-bearing can rot
inside a dependency nobody controls.

**Step 3: make invalid states unrepresentable.** The application
checks rules for good error messages; the database constraint is the
guarantee:

- `enrolled_count <= capacity` is a CHECK constraint, not a promise.
- Every section has a capacity row by trigger, created in the same
  migration — and a missing row fails *loudly and distinctly*, never
  as a fake "section is full."
- Audit rows are written in the SAME transaction as the change they
  describe. A crash cannot produce an unaudited change; there is no
  "between" to crash in.
- Institution scoping is in unique constraints, not just WHERE
  clauses, so no code path can accidentally cross tenants.

**Step 4: the frontend obeys the same philosophy** (full rules in
`FRONTEND.md`). Server-rendered pages with real URLs; Alpine (CSP
build) + fragment swaps for in-page mutations; every fragment
interaction also works with JavaScript off. Three animated surfaces
exist, durations come from tokens, `prefers-reduced-motion` disables
all of it. And the load-bearing rule: **optimistic UI is forbidden on
truthful operations** — registration, grades, documents show
`Submitting…` and then the *committed server outcome*. A false
"Enrolled" painted by JavaScript is the same lie as a false commit.
Static assets are content-hash fingerprinted and served immutable for
a year — the browser's native cache, zero service-worker machinery;
page *content* is `no-store` (test-pinned), because cached truth goes
stale and stale truth about seats and grades is a lie.

---

## Part III — The Constraints (correctness before speed)

**The last-seat problem, worked in full.** Two students, one seat,
same instant. The naive algorithm:

```
1. SELECT enrolled_count, capacity ...     -- both read 29 < 30
2. both requests pass the check
3. UPDATE ... SET enrolled_count = 30      -- both write
   ⇒ 31 students in 30 seats. Oversold.
```

Read-check-write is *algorithmically* wrong under concurrency. The fix
is not locking advice or infrastructure — it is making the check and
the write one atomic statement:

```sql
UPDATE section_capacity
   SET enrolled_count = enrolled_count + 1
 WHERE section_id = $1
   AND enrolled_count < capacity          -- the check IS the write
RETURNING enrolled_count;
```

Zero rows = full (or a registrar override is claimed — a real record:
who, why, which rule, consumed by which enrollment). One row = the
seat is yours; the row lock forced every competitor to re-evaluate
against the new count. The proof is a test that races two
registrations for the last seat and asserts exactly one success. That
test is the architecture's load-bearing wall; it stays green in every
phase, and honest 409s under contention were re-proven at load
(69 % denial rate, zero oversell) during this benchmark cycle.

**Registration is one transaction with a fixed lock order**
(student-term row → enrollment/section state, shared by register and
drop so they cannot deadlock each other), carrying an idempotency key
end to end: a crash after commit but before the response, followed by
a browser retry, returns the *original* receipt instead of
double-registering. The suite proved this accidentally: a load script
once seeded identical keys across threads and the server correctly
collapsed 2,683 requests into 671 rows.

**Durability as a floor.** Measured on this workstation:

```
raw single-row INSERT; COMMIT in psql:      60–94 ms  (the fsync)
full registration through the whole stack:  57–63 ms
   ⇒ application overhead over physics: ~zero
```

Session resolution, CSRF, seven policy checks, the seat reservation,
the enrollment insert, the audit row, the commit, and the response
render — all inside the noise of one disk sync. That is the real
definition of non-pessimized: *the code costs nothing beyond what the
problem irreducibly costs.*

**The security model in one paragraph** (deep-dive: `SECURITY.md`,
every row test-backed): opaque high-entropy tokens, hash-only at rest;
HttpOnly/SameSite always, Secure in production; rotation on login and
privilege change; idle + absolute expiry; revocation on logout,
suspension, and password change — *immediate*, a property the moka
experiment below nearly traded away. Argon2id with OWASP parameters,
login throttling on an (account, IP) window that self-expires so it
cannot become a lockout weapon. CSRF middleware on every browser
mutation; login is the sole exemption. Parameterized SQL everywhere;
errors carry `{"code","message"}` and never SQL or secrets.

---

## Part IV — The Performance Ladder, As It Actually Happened

The catalog endpoint went 3,940 → 13,951 req/s with **zero**
infrastructure. The ladder, step by step, with the arithmetic. (Every
run: quiet machine verified, warm caches declared, dataset stated,
p50/p90/p99 + errors + transfer recorded — see `PERFORMANCE.md` for
full context. The three benchmark classes — in-process, read path,
durable write — are never conflated.)

### Step 0: Baseline honestly

```
t8/c64:  3,940 req/s,  p50 15.4 ms
```

Two lessons about honest baselines were paid for during this work: a
leftover server process once halved the numbers, and an unrelated
app's connection pool once blocked startup entirely (96 of PostgreSQL's
100 default slots). Check the machine before trusting any local run.

### Step 1: Right algorithm — twice

**1a. The session-touch stampede.** All 64 in-flight requests on one
session read the same stale `last_seen_at` and each paid a ~68 ms
durable commit to write the same timestamp — p99 474 ms on a pure read
path. Fix: repeat the staleness guard *inside* the UPDATE so only the
first writer matches, and `FOR NO KEY UPDATE SKIP LOCKED` so the herd
doesn't even wait:

```
3,940 → 7,633 req/s   (+94 %)
```

**1b. The LIMIT that came too late.** `EXPLAIN ANALYZE` showed the
catalog computing the meeting aggregate and sort for all ~280 open
sections, then keeping 20:

```
work done:   280 × (LATERAL aggregate + wide row)
work needed:  20 × (LATERAL aggregate + wide row) + one skinny sort
```

Rewrite: paginate skinny `(code, section_code, id)` rows first, join
the expensive parts onto the 20 winners:

```
query alone:  2.94 ms → 0.79 ms       (3.7×)
end to end:   7,633 → 13,951 req/s    (+83 %)
p99 at c64:   423 ms → 8.9 ms         (47×)
```

The p99 collapse teaches the deepest lesson here: cheaper queries mean
fewer runnable backends fighting for cores, so the tail — which is
pure queueing — vanishes with the waste. Latency tails are usually a
queue symptom, not a code symptom.

### Step 2: Non-pessimization — count what actually happens

Transactions per catalog request, measured via `pg_stat_database`
counters: **2.2** (one session resolve, one catalog query, background
slivers). A recorded plan to "fold the session touch into resolve"
turned out already moot — the stampede fix made the touch fire once
per 60 s, not per request; the lever was struck from the docs *with
the measurement attached*. The registration write path got the same
census (Part III's 57–63 ms): **zero pessimizations found, zero
changes made.** Changing nothing, on evidence, is a first-class
result — optimization without a measured target is how codebases
accrete clever unreadable code that saves nothing.

### Step 3: Optimization experiments — and the discipline of reverting

**The cache that won twice and stayed out anyway.** An in-process
moka cache on session resolution, 5 s TTL:

```
2026-07-25 (pre-rewrite):   predicted ≤8 %, measured +7.8–8.2 %  (at ceiling)
2026-07-30 (post-rewrite):  measured +20.9 % (c64), +22.4 % (c16)
```

The second measurement matters pedagogically: after the catalog got
3.7× cheaper, the session lookup became a *bigger share* of each
request, so the cache's win nearly tripled — and **the answer stayed
no**, because the disqualifiers were never about the size of the win:

- A cache hit skips `revoked_at`, account status, `session_version`.
  Every present and future revocation path must remember to
  invalidate; forgetting **fails open** — a suspended user keeps an
  authenticated session for the TTL. Six revocation-immediacy tests
  caught this on contact.
- Measured two-instance divergence: logout through instance A leaves
  instance B serving the dead session for the full 5.005 s TTL.
  In-process session caching is structurally disqualified for any
  scaled-out future.
- Nothing needs the win: 13,951 req/s against ~200 req/s of real load.

A bigger bribe does not make a bad trade sound. The verdict lives in
ADR-13 with an explicit **adopt trigger** (>1 instance AND session
resolution a measured bottleneck), the implementation survives on
`exp/moka-session-cache`, and the re-measure reproduces with one
cherry-pick. This is how to say "no" without losing the work.

**Knowing where the ceiling is.** Measure the layers separately:

```
SELECT 1 through the wire, 24 clients:     604,000 stmts/s
the real catalog query alone (pgbench):     23,261 req/s @ 1.03 ms
the full app:                               13,951 req/s
```

Protocol overhead: non-factor (69 k of a 604 k budget). Database-side
ceiling: ~23 k. The app sits at 59 % of it; the gap is the
security-mandatory session resolve plus the app sharing 12 cores with
PostgreSQL. No code change closes that gap honestly — which is the
exit condition for step 3 and the entry condition for step 4. You are
only allowed to buy hardware once you can write this table.

### Step 4: Infrastructure — sized by arithmetic, not vibes

**Pool size** (24 hardware threads):

```
pool 15:  2,940 req/s   p99 24.6 ms   (starved)
pool 64: 13,951 req/s   p99  8.9 ms   ← default
pool 96: 13,834 req/s   p99 11.6 ms   (wash — and 4 slots from the
                                       max_connections cliff we hit live)
```

Pools are a concurrency valve, not a performance dial.

**Overload shape** — 1,000 connections against the pool of 64:

```
throughput:  13,795 req/s (unchanged)
p50:         72.0 ms  = closed-loop prediction: 1000 ÷ 13.8k ≈ 72 ms
p99:         77.5 ms  — 5.5 ms above p50. A fair queue, not a cliff.
errors:      0 in 415,199
```

If overload p50 doesn't match `concurrency ÷ throughput`, something is
thrashing.

**The topology measurement that settles the argument.** Same app, same
data, database moved 53 ms away:

```
4–6 sequential round trips × 53 ms ≈ 215–325 ms/request
closed-loop: 64 ÷ 0.325 s ≈ 197 req/s predicted
measured:                     194 req/s   (−96 %, both machines idle)
```

The app and its database belong on the same host or sub-millisecond
network, always (now a standing rule in `OPERATIONS.md`). If a hop
ever becomes mandatory, cut round trips first — the wire dominates
everything else.

**Writes are priced by physics.**

```
one fsync on this SSD:            ~68 ms  ⇒ ~15 sequential commits/s
16 concurrent students measured:  117.8 committed registrations/s
                                  (group commit: ~8 tx share an fsync)
verification: 3,536 requests = 3,536 audit rows. Zero fake successes.
```

~118 real registrations/s = all 35,000 registration-day writes in five
minutes, on a workstation. The remaining lever is storage hardware
(~10×, zero code). That is what finished software looks like: the
performance roadmap reduces to a shopping list.

### The ladder, scored

```
Step 1 (algorithm):         3,940 → 13,951 req/s;  p99 423 → 8.9 ms
Step 2 (non-pessimization): censuses confirmed nothing left; levers struck
Step 3 (optimization):      +21 % measured available — refused on
                            structural security cost, trigger recorded
Step 4 (infrastructure):    pool sized by math; naive scale-out priced
                            (−96 %); storage identified as the one lever
```

The entire 3.5× came from steps 1–2: reading plans, counting
statements, deleting waste. Steps 3–4 mostly produced *documented
refusals* — and those refusals, with their numbers and re-entry
triggers, are why this system will still be simple in five years.

---

## Part V — Operating It, and Future Changes

**Running it** (details: `../../README.md`, `../../RUN.md`,
`OPERATIONS.md`): one binary, env-var config validated at startup
(startup aborts on invalid values and never echoes them), migrations
apply automatically, the server refuses to serve without a valid
license row (seed provides one), the document worker and stale-job
reaper run inside the binary, and the deployment topology rule is
measured law: app and database on the same host or sub-millisecond
network. Backups: PostgreSQL is the single source of truth plus the
document storage directory — drill in `BACKUP_AND_RESTORE.md`.

**The maintenance constitution** (`../../CLAUDE.md` — read it before
changing anything): every slice lands with `fmt` + `clippy -D
warnings` + full tests green and a commit naming what became true; a
red gate never proceeds; every deviation from the design docs gets a
numbered ADR; every benchmark records its context; every abandoned
experiment keeps its branch, numbers, and re-entry trigger. Business
policy unknowns: choose the conservative default (fail closed,
preserve academic integrity), make it configurable, write the
assumption down, keep moving.

**Recorded triggers** — future maintainers act on conditions, not
archaeology:

| Trigger (measured condition) | Prepared response |
|---|---|
| 1,000+ open sections per term, or catalog p50 regresses | LIMIT-first rewrite is in; next is keyset pagination, only if deep OFFSET shows in profiles |
| >1 app instance AND session resolution a measured bottleneck | Shared-store session cache from the experiment branches (delete-on-revoke, fall-through-on-unreachable), re-priced against real network RTT — never the in-process cache (5.005 s divergence, measured) |
| Write throughput needed beyond ~120/s | Server-grade NVMe: ~10× for zero code |
| Database CPU saturates on reads | More cores; the query work is already minimal (0.79 ms, plan-inspected) |
| A mandatory network hop app↔DB | Cut round trips first; the wire is −96 % |

---

## Part VI — The Transferable Method (the tutorial in 12 lines)

```
 1. Put real numbers on the load before designing.
 2. Find the invariants; they are the actual problem.
 3. Modules first; processes only when organizationally forced.
 4. Own your load-bearing code; depend only for encoded expertise.
 5. Make invalid states unrepresentable (constraints, triggers,
    same-transaction audit).
 6. Make racy checks atomic writes; prove it with a race test.
 7. Measure the physics floor (fsync, RTT); build against it.
 8. Optimize in order: algorithm → waste-removal → experiments →
    hardware. Never skip ahead.
 9. Pre-register predictions; measure; REVERT wins whose costs are
    permanent. Record the adopt trigger. (A win that doubles is
    still refused if the cost is structural.)
10. Know your ceiling per layer; buy hardware only when the gap to
    the ceiling is security or physics.
11. Under overload, verify latency = concurrency ÷ throughput;
    anything worse is thrash.
12. Ship every conclusion with the number that proves it, in a doc
    the next person will actually find.
```

---

## Part VII — Books That Deepen Each Part

Curated, not exhaustive — each mapped to the part it extends.

**The mindset (Parts 0–II: simplicity, longevity, owning your code)**

- **"A Philosophy of Software Design" — John Ousterhout.** Deep
  modules with small interfaces; complexity as incremental debt. The
  module-not-microservice choice is Ousterhout applied.
- **"The Practice of Programming" — Kernighan & Pike.** Old, short,
  permanently right: simplicity, clarity, and boring algorithms that
  are correct on edge cases.
- **"The Pragmatic Programmer" (20th Anniversary) — Hunt & Thomas.**
  Orthogonality, tracer bullets, small reversible steps — the
  commit-per-slice rhythm.
- **"Software Engineering at Google" — Winters, Manshreck, Wright.**
  "Software engineering is programming integrated over time." Its
  dependency chapter is the strongest published argument for the
  handmade rule.

**Correctness under concurrency and failure (Part III)**

- **"Designing Data-Intensive Applications" — Martin Kleppmann.** The
  most valuable book on this list: transactions, isolation,
  replication, and exactly the read-check-write races worked here.
- **"Transaction Processing" — Jim Gray & Andreas Reuter.** The
  classic behind every claim about durability, group commit, and
  locking. Skim once, consult forever.
- **"Release It!" (2nd ed.) — Michael Nygard.** Pools, timeouts,
  backpressure — why overload must become a queue and not a cliff,
  as war stories.

**Performance and non-pessimization (Part IV)**

- **"Systems Performance" (2nd ed.) — Brendan Gregg.** The measurement
  discipline: knowing whether you are CPU-, I/O-, or queue-bound
  before touching anything. The layer-ceiling table is this book's
  method in miniature.
- **"Computer Systems: A Programmer's Perspective" — Bryant &
  O'Hallaron.** Why a loopback round trip is ~0.05 ms and an fsync is
  ~10,000× that — the cost intuition everything here rests on.
- **"Data-Oriented Design" — Richard Fabian.** Performance comes from
  shaping data for the machine — the skinny-rows-first rewrite,
  applied to memory instead of SQL.

**Databases (Parts III–IV)**

- **"SQL Performance Explained" — Markus Winand.** The best
  return-per-page here: indexes, join order, and why LIMIT placement
  decides how much work a query does — literally the Part IV.1b
  rewrite, generalized. Free companion: use-the-index-luke.com.
- **"PostgreSQL 14 Internals" — Egor Rogov.** Free PDF: buffers, WAL,
  locks, vacuum, the planner — everything `EXPLAIN (ANALYZE, BUFFERS)`
  assumes you know.
- **"Database Internals" — Alex Petrov.** B-trees, LSM trees,
  replication consensus — down the stack where Kleppmann goes across.
- **"The Art of PostgreSQL" — Dimitri Fontaine.** Pushing logic into
  SQL where it belongs; the atomic seat reservation is this book's
  philosophy in one statement.

**Starting from zero:** Ousterhout → Kleppmann → Winand → Gregg, then
the rest as the problems arrive. Four books and you can re-derive most
of this document yourself — which is the point.

---

*Everything above is checkable: benchmarks with full context in
`PERFORMANCE.md`, security claims test-named in `SECURITY.md`, every
permission cell test-backed in `PERMISSIONS.md`, experiments alive on
`exp/moka-session-cache` and `exp/redis-session-cache`, the last-seat
race test in every CI run, and the load scripts in `../load/`
reproducing every number on your own hardware. Distrust any document
like this one that doesn't end with a sentence like that.*
