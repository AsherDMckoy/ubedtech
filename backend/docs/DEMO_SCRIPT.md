# Demo script — the registrar journey, then four student/staff journeys

Rehearsed 2026-07-22; Journey R added 2026-07-28. Every step below is
backed by a green acceptance test; nothing in this script depends on
luck or timing.

Lead with Journey R if the audience cares about the institutional/
administrative story — it sets the rules every later journey lives
inside. If the audience is student-experience-first, run it after
Journey 1 instead; it stands alone either way.

## Setup (once)

```sh
createdb ubedtechdb                       # if it does not exist
cd backend
psql -d ubedtechdb -f src/dev/seed.sql    # idempotent demo dataset
cargo run --release                       # migrations apply at startup
```

Optional, recommended: `cargo run --release -- seed-demo` layers the
deterministic full-scale dataset (900 students, ~280 current sections)
on top, so every screen shows real volume. The scripted scenarios below
survive it untouched — but the catalog then paginates (~280 sections,
20 per page, ordered by course code), so surface each scripted course
by typing its code into the catalog search instead of expecting all
four on the first page.

All demo passwords are `ub-demo-password`.

| Account | Roles | Used for |
|---|---|---|
| `demo.student` | student | journeys 1–4 (the "ordinary user") |
| `demo.held` | student (advising hold) | the blocked-registration aside |
| `demo.instructor` | instructor | journey 2, grade entry |
| `demo.registrar` | registrar + records officer + document officer | journey R, journeys 2–3 staff side |
| `demo.admin` | institution admin | optional detour (calendar/settings/accounts) |
| `demo.platform` | platform licensing admin | journey 4 |

Seeded fodder (term FALL-2026, add/drop open): CMPS-2131 **full** with a
mixed roster (published/draft/blank grades), CMPS-3141 **open** (register
against this), MATH-3201 **3 seats left**, PHYS-2101 **blocked by an
unmet prerequisite**, and one **pending** transcript request from
demo.student. demo.student also carries a Spring 2025 history (A / B / F,
all published), which powers two more catalog states: CMPS-1121
**already passed → register denies** ("course already completed with a
passing grade"), MATH-1151 **failed → retake allowed**.

demo.student is also mid-semester in two graded courses: CMPS-2515 has a
**published B+** (the Grades page shows it as current standing — the
lecturer grades into the system during the term, so students watch their
progress), and STAT-2101 has a **draft A-** the student cannot see —
publish it live (as `demo.registrar`) for a second, quicker version of
the journey-2 boundary flip.

Optional before starting: turn JS off in devtools for any journey — every
mutation below is a plain form POST and behaves identically minus the
enhancements. That is the demo's strongest claim; consider doing journey
1 with JS off and saying so.

## Journey R — registrar: the rules the university runs on

Open with: "Everything you're about to see students and lecturers do
happens inside rules a registrar sets. Here's that side."

1. Sign in as `demo.registrar` → registrar overview. **Point out**: this
   is a different density on purpose — where the student view is calm and
   guided, this is a dense operational console: the four term tiles
   (sections, seats filled, needs-attention, active holds), the
   needs-attention worklist, the window-status panel, and the sortable/
   filterable sections table. Same design system, expert density
   (FRONTEND.md §6 made visible — worth saying to a technical evaluator).
2. Terms & windows. **Point out**: registration and drops share a single
   add/drop window that opens and closes together — a deliberate policy
   decision (ADR-8). Show the current term's window open. Optionally
   open "Edit windows" and save a change — it commits on the server and
   is audited in the same transaction; no optimistic "saved".
3. Sections & capacity. Filter the sections table to a course, open a
   section. **Point out**: capacity lives here, and a section always has
   a capacity row (created by trigger in the same transaction as the
   section — a missing row can never masquerade as "full"). Adjust
   capacity and save. Tie it back: "the seat counts the student saw are
   this number, live."
4. Holds. Open `demo.held`'s student page (Students → search). **Point
   out the connective tissue**: "This is exactly the hold that blocks
   that student from registering — they see it named inline, and this is
   where it comes from. Nothing hidden on either side." Place/release a
   hold on another student to show the form; both commit audited.
5. Overrides. From a student's page, grant an override (prerequisite or
   capacity), then show the Overrides review list. **Point out**: an
   override is an explicit, recorded grant — who authorized it, which
   rule, why, when it expires, and which enrollment consumed it.
   Single-use, audited. "There are no hidden admin bypasses in this
   system; a rule can only be waived on the record, with a name
   attached." (A genuine trust-and-integrity beat — lean on it.)
6. Optional close: back to the overview's needs-attention worklist —
   full sections, unassigned instructors. "This is what a registrar
   opens each morning. The system surfaces the work, it doesn't just
   store data."

Connective payoff, said out loud: "So when you watch the student
register in a moment, they're registering into a window this person
opened, against a capacity this person set — and if they're blocked,
it's by a hold or a prerequisite this person can see and override, on
the record. Three roles, one system."

## Journey 1 — student: register → schedule → drop

1. Sign in as `demo.student`. **Point out**: the dashboard is complete
   HTML on first paint — enrollments, add/drop deadline, events; no
   spinners, no client fetch.
2. Catalog. **Point out the four seeded states in one screen**: open
   seats (CMPS-3141), low seats (MATH-3201 — count in words, not a color),
   full (CMPS-2131 — an honest disabled button, not a hidden one),
   prerequisite-blocked (PHYS-2101 — the reason is named in the row).
3. Register for CMPS-3141. **Point out**: the button says "Checking…"
   while the server decides — never an optimistic success — then the row
   swaps to the enrolled state with the seat count updated.
4. Schedule: the new section is on the grid with its meeting times.
5. Registration page → Drop CMPS-3141. Confirm, seat count returns.
6. Aside (30 s): sign in as `demo.held`, try to register — refused with
   the advising hold named inline. Nothing hidden, nothing vague.

## Journey 2 — instructor: grade → publish → student sees only published

1. Sign in as `demo.instructor` → Sections → CMPS-2131 roster.
   **Point out**: the roster is a mix of published, draft, and not-entered
   grades — status is always a badge *plus the word*.
2. Enter/adjust a draft grade for a student. Saved = committed; the page
   re-renders from the database, not from client state.
3. In a second window sign in as `demo.student` → Grades. **Point out**:
   the draft grade is *absent* — students see published grades only, and
   that boundary is enforced in the service (a test races it).
4. Sign in as `demo.registrar` (records officer) → same roster → Publish
   section grades. **Point out**: publish flips only at the server commit;
   the test suite proves it never flips early.
5. Refresh the student window: the grade is now visible, with history.

## Journey 3 — documents: request → approve → generate → download

1. As `demo.student` → Documents. A transcript request is already
   pending (seeded); or submit a new one — **point out the idempotency
   key in the form**: double-submitting returns the original request
   instead of creating a twin.
2. Sign in as `demo.registrar` (document officer) → Document queue →
   Approve the pending request.
3. Back as the student: the row shows the honest pipeline state
   (approved → generating → ready) — the row polls and swaps in the
   server-rendered fragment; with JS off, reload does the same.
4. Download the PDF. **Point out**: authorization is on the download
   route itself (owner or officer — a test proves other students 403),
   and the artifact's checksum is re-verified on every download.

## Journey 4 — license: disable → locked out → carve-outs → restore

1. Sign in as `demo.platform` → `/ui/platform/license`. **Point out**:
   status, validity window, and the full change history with who/why.
2. Suspend with a reason (reason is mandatory; change + audit commit in
   one transaction).
3. In the student window, click anything: **redirected to the
   "Access suspended" screen** — it says no individual account is
   disabled and nothing is lost.
4. Show the carve-outs while locked: `/license/status` still answers,
   sign-in still works, and the platform panel itself is reachable —
   that is how an operator recovers a locked deployment.
5. Reactivate. The student window resumes exactly where it was —
   **no one was signed out** (the lock is the license, not the users).

## If asked

- "What happens on a double-click of Register?" — one enrollment; the
  registration POST carries an idempotency key and the seat reservation
  is a single conditional UPDATE (the two-clients-one-seat race is a
  standing test).
- "Self-hosted?" — the same panel renders read-only; license state then
  changes only via a platform-signed license file import, enforced in
  the service.
- "Accessibility?" — axe over all 30 pages in CI plus structural
  assertions in every flow test; the manual screen-reader pass is the
  remaining human step (docs/FRONTEND_DESIGN_SYSTEM.md checklist).

- "Is there a mobile app?" — exact framing, honest and strong:

  "There's no separate mobile app today, and that's deliberate for this
  stage — but the important part is what makes one cheap when we want
  it. The backend is API-first: every action you've seen — sign in,
  register, drop, view schedule, request a document — is a server
  endpoint with the business rules enforced server-side, in the database
  transaction. The seat-reservation logic, the licensing, the
  authentication, the audit trail — none of that lives in the web page;
  the page is just one client of the API. So a native iOS or Android app
  would be an additional *client* of the same endpoints, not a rewrite
  of the system. The hard part — correctness under concurrency,
  security, the institutional rules — is already built and already
  tested. Mobile is additional surface, not additional foundation."

  If pressed on timeline: "It's a real project — a native app is its own
  codebase and app-store cycle — but it starts from a finished, tested
  API rather than from scratch, which is the expensive part most teams
  underestimate."

  Do NOT imply a mobile app exists, is in progress, or is trivial. The
  strength of the claim is that it's true, and that only holds if it's
  stated as a natural extension, not a near-done feature.

- "Can it host course content / act like Moodle or Canvas?" — same
  discipline: name it as a natural Phase-2 direction the existing
  document-generation and storage patterns extend toward, but do not
  demo it, imply it exists, or build any of it this round. The sharp
  story is "the university's system of record, done correctly," not
  "we also do everything Canvas does."
