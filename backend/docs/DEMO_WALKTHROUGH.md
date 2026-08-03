# Demo walkthrough — full guide

A complete script for demoing the UB EdTech platform: setup, credentials,
three timed journeys (student → instructor → registrar), the supporting
cast, and recovery notes. Written to be read at the podium.

---

## 1. Before the demo (one command)

Containers (podman or docker — the demo-ready path):

```sh
./demo.sh            # build + launch app and PostgreSQL, wait, smoke-check,
                     # print credentials — ready at http://127.0.0.1:8080
```

`./demo.sh fresh` wipes the database and relaunches (known-good stage in
about a minute — the first build takes longer). `./demo.sh down` stops
everything. Stack definition: `compose.yaml` + `backend/Containerfile`;
the app container migrates and seeds the full demo dataset by itself on
first start.

Bare metal (local PostgreSQL, no containers):

```sh
cd backend && cargo build --release
DATABASE_URL=postgresql://<user>@localhost:5432/ubedtechdb \
    ./target/release/backend seed-demo        # migrates + seeds, idempotent
DATABASE_URL=postgresql://<user>@localhost:5432/ubedtechdb \
    ./target/release/backend                  # serves http://127.0.0.1:8080
```

Checklist:

- [ ] `./demo.sh` printed READY (it already smoke-checked a real sign-in).
- [ ] Sign in as `demo.student` — dashboard shows classes + deadline.
- [ ] Sign out. Zoom the browser so the last row is comfortable (90–100%).
- [ ] Close other tabs; the nav swap keeps the demo fast, but a clean
      window keeps it calm.

**Resetting between runs:** the dataset is idempotent — anything you
change during a run (a registration, a drop, a draft grade, a released
hold) persists. Either demo the changes forward (register a *different*
open section next run) or `./demo.sh fresh` (bare metal: drop the
database and re-run `seed-demo`). Grades you publish stay published;
pick a different not-entered student each run.

## 2. Credentials

Every password is `ub-demo-password` (development seed only).

| Journey | Username | Shown as | Roles |
|---|---|---|---|
| 1 | `demo.student` | Dana Castillo | student (clean record) |
| 1b | `demo.held` | Marlon Usher | student (advising hold) |
| 2 | `demo.instructor` | Alba Flores | instructor |
| 3 | `demo.registrar` | Renee Garbutt | registrar + records officer + document officer |
| extra | `demo.admin` | Iris Novelo | institution admin |
| extra | `demo.platform` | Platform Operations | platform licensing |

Seeded stage (term FALL-2026, add/drop open):

- **CMPS-2131** — full (4/4), roster mixes published / draft / not-entered
- **CMPS-3141** — open seats (the section to register live)
- **MATH-3201** — 3 of 25 seats left (low-seats warning)
- **PHYS-2101** — full AND blocked by unmet prerequisite MATH-2110
- One **pending official-transcript request** from Dana, already in the queue

---

## 3. Journey 1 — Student (`demo.student`, ~7 min)

The story: *everything a student needs — registration, schedule, grades,
documents — in one calm place, and the UI never lies about state.*

1. **Sign in.** Point out the front door: form on the left works before
   anything else loads; the right column is pure CSS — no image request.
2. **Dashboard.** Walk the zones: today's classes with honest
   "Registered" badges, the one gold number (add/drop deadline), GPA
   history, mini calendar, campus events.
   - *Moment:* hover "Midterm examinations" in Campus events — the mini
     calendar jumps to October and lights the span. Keyboard focus does
     the same (Tab to it).
3. **Header.** Click the avatar (DC): name, email, student ID, programme,
   standing. Flip the theme with the switcher beside it — instant, and it
   persists (server cookie, not just JS).
4. **Catalog.** The headline surface:
   - Type `cmps` — results refresh live from the server; the URL updates,
     so the search is shareable/reloadable.
   - Hover a row — it expands in place: prerequisites, room, instructor,
     credits.
   - Click a course *name* — the course dialog: description, faculty,
     full facts.
   - **Register for CMPS-3141.** The button says "Checking…" until the
     server commits, then the row swaps to its enrolled state. Say it:
     *no optimistic success — that row is the database answering.*
   - Scroll to **PHYS-2101**: blocked with the reason named (unmet
     prerequisite). **MATH-3201**: low-seats warning. Full sections:
     honest disabled button, never gold.
5. **My courses.** The registration you just made, credit total, course
   dialog on click, the "remaining this semester" list. Drop something
   only if you want to show the confirmation dialog — it costs a seat.
6. **Schedule.** The week as a timeline (days across, time down).
   - *Moment:* hover a class card — its remaining meeting dates light
     gold on the month calendar. Click a marked day — the detail card
     fills in the aside without a page load.
7. **Grades.** One page: this term's published grades on top, the full
   published record and transcript snapshots beneath.
8. **Documents.** The pending transcript request (leave it — journey 3
   approves it). Open **Unofficial transcript** — print-ready, chrome
   drops out in the print stylesheet.
9. **The hold story (30 s, optional).** Sign out → `demo.held`:
   registration is blocked with the advising hold *named*. Say it:
   *button visibility is not authorization — the server enforces every
   rule the UI shows.*

## 4. Journey 2 — Instructor (`demo.instructor`, ~3 min)

The story: *role-aware from the first pixel; grade entry is safe by
construction.*

1. **Sign in.** The rail shows only "Your sections" — nav items exist
   only for roles that hold them (and the account menu says Role:
   Instructor).
2. **Your sections.** CMPS-2131 (4/4) and CMPS-3141 with live enrollment
   counts — including the seat Dana just took.
3. **Roster (CMPS-2131).** Three unmistakable row states: green
   published (read-only), amber draft (entered, invisible to students),
   muted not-entered.
4. **Enter a grade** for a not-entered student and save — the row turns
   amber. Say it: *drafts never leak; a student sees nothing until the
   records office publishes.*
5. Note what you *can't* do: there is no publish button here.
   Publishing is a records-office power — that's journey 3.

## 5. Journey 3 — Registrar (`demo.registrar`, ~5 min)

The story: *the institution's control room — capacity, holds, overrides,
publishing, documents — every action audited.*

1. **Sign in.** Full rail: Overview, Terms & windows, Sections, Courses,
   Students, Overrides, Document queue (this account also holds the
   records- and document-officer roles).
2. **Overview.** Metric tiles, the needs-attention worklist (full and
   near-full sections), sortable dense table — click a column header.
3. **Terms & windows.** FALL-2026's registration / add-drop / grade-entry
   windows — the same dates every student surface enforces.
4. **Sections.** Raise CMPS-2131's capacity above 4 — it opens up in the
   catalog. Then try to shrink it below current enrollment: refused.
   Say it: *the database constraint is the guarantee; seats cannot be
   oversold, we proved it with a concurrency test.*
5. **Roster → publish.** Open CMPS-2131's roster from Sections: as a
   records officer this account CAN publish. Publish the drafts —
   confirm dialog, then the rows lock green. (Re-sign-in as
   `demo.student` later to show the grade arriving, if time allows.)
6. **Students.** Look up Marlon Usher — the advising hold from journey
   1b. Release it; his registration unblocks.
7. **Overrides.** Grant a prerequisite override (who, why, expiry
   recorded) — PHYS-2101 opens for that student.
8. **Document queue.** Dana's pending transcript request: approve it.
   The PDF renders in the background job worker — never on a request
   thread — and Dana's Documents page polls the same truth to "ready".

## 6. Supporting cast (if asked)

- **`demo.admin`** — institution calendar (the campus events on every
  student surface), settings, accounts.
- **`demo.platform`** — flip the UB license inactive: every UB request
  answers the 402 locked screen; flip it back. The licensing gate sits
  outside the session, before any database work.

## 7. If something goes sideways

- **Blank/odd page:** hard-refresh (Ctrl-Shift-R). Assets are
  fingerprinted; a stale tab is the only cache that can exist.
- **"Section is full" on the section you meant to register:** a previous
  run took the seat — use any other open section, or re-run `seed-demo`.
- **Login page looks wrong:** it can't be the rail cookie anymore (fixed
  and pinned); check the server is actually running: `ss -tlnp | grep 8080`.
- **Server died:** containers restart themselves (`restart: on-failure`);
  bare metal, rerun the serve command from §1. Sessions survive in the
  database — the browser stays signed in either way.
- **Everything on fire:** `./demo.sh fresh` = known-good stage in about
  a minute.
