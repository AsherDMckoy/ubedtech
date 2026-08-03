# Demo walkthrough — three journeys

Demo credentials (development seed only — `src/dev/seed.sql`; every
password is `ub-demo-password`):

| Role | Username | Shown as |
|---|---|---|
| Student | `demo.student` | Dana Castillo |
| Student with a hold | `demo.held` | Marlon Usher |
| Instructor | `demo.instructor` | Alba Flores |
| Registrar (+ records & document officer) | `demo.registrar` | Renee Garbutt |
| Institution admin | `demo.admin` | Iris Novelo |
| Platform licensing | `demo.platform` | Platform Operations |

Run the app (`DATABASE_URL=… ./target/release/backend`), open
`http://127.0.0.1:8080/ui/login`. All three journeys below assume the
seeded FALL-2026 term with add/drop open.

## Journey 1 — Student (`demo.student`)

1. **Sign in.** Split front door: form left, brand gradient right.
2. **Dashboard.** Today's classes with Registered badges, the add/drop
   deadline as the page's one gold number, GPA history, the six-row mini
   calendar (hover a campus event — the calendar jumps to its month and
   lights the span), campus events.
3. **Account menu.** Top-right avatar (DC): name, email, student ID,
   programme, standing; switch theme light/dark here; sign out lives here
   too.
4. **Catalog.** Type in the search box — results refresh live from the
   server, URL stays shareable. Hover a row: it expands in place with
   prerequisites, room, instructor, credits. Click a course name: the
   course dialog with description and faculty. Register for
   **CMPS-3141** (open seats) — the row swaps to its committed state, no
   optimistic lie. Try **PHYS-2101** — honestly blocked (unmet
   prerequisite MATH-2110). **MATH-3201** shows the low-seats warning.
5. **My courses.** Registered list (same course dialog on click), credit
   total, the "remaining this semester" section, drop with confirmation
   dialog.
6. **Schedule.** The week as a timeline — days across, hours down; hover
   a class card and its remaining meeting dates light gold on the month
   calendar aside; click a marked day for its detail; month prev/next
   swap in place.
7. **Grades.** One page: this term's published grades on top, the full
   term-by-term academic history and transcript snapshots beneath.
8. **Documents.** Request an official transcript; the row polls honestly
   (pending → processing → ready) — the seeded request from Dana is
   already in the officer's queue. Print-ready unofficial transcript and
   proof of enrollment.
9. **The hold story (optional).** Sign out, sign in as `demo.held`:
   registration is blocked with the advising hold named — button
   visibility is not authorization.

## Journey 2 — Instructor (`demo.instructor`)

1. **Sign in.** The rail shows only "Your sections" — nav is role-aware,
   and the account menu shows Role: Instructor.
2. **Your sections.** CMPS-2131 (full, 4/4) and CMPS-3141 with
   enrollment counts.
3. **Roster (CMPS-2131).** Three visually distinct row states: published
   (green, read-only), draft (amber — entered but invisible to
   students), not entered (muted).
4. **Enter a grade.** Pick a grade for an un-entered student, save —
   the row turns draft. Nothing reaches the student yet.
5. **Publish.** The publish action confirms via dialog, then commits;
   published rows lock. Re-sign-in as `demo.student` to show the grade
   appearing — the write path is server-committed truth, never
   optimistic.
6. **Grade history.** Every version of a changed grade, who and when —
   the audit story in one screen.

## Journey 3 — Registrar (`demo.registrar`)

1. **Sign in.** Full registrar rail: Overview, Terms & windows,
   Sections, Courses, Students, Overrides, Document queue (this account
   also holds records- and document-officer).
2. **Overview.** Metric tiles, needs-attention worklist (full and
   near-full sections), window badges, dense sortable sections table
   (click column headers).
3. **Terms & windows.** FALL-2026 with registration/add-drop/grade-entry
   windows — the dates the student pages enforce.
4. **Sections.** Capacity management: raise CMPS-2131 above 4 and show
   it opening up in the catalog; try shrinking below current enrollment
   — refused (the constraint holds, no oversell).
5. **Students.** Look up Marlon Usher (`demo.held`), see the advising
   hold; release it and show his registration unblock, or place one on
   another student.
6. **Overrides.** Grant a prerequisite override (who, why, expiry) —
   then PHYS-2101 opens for that student.
7. **Document queue.** Dana's pending transcript request: approve it and
   watch the status move — the student's Documents page polls the same
   truth. The PDF renders in the background job worker, never on a
   request thread.

Supporting cast: `demo.admin` (institution calendar — the campus events
on every student surface — settings, accounts) and `demo.platform`
(flip the UB license inactive: every UB request answers the 402 locked
screen; flip it back).
