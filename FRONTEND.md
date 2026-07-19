# Frontend Design Constitution — University Education Platform

The frontend equivalent of `CLAUDE.md`. These rules decide the arguments in
advance so no screen has to relitigate "should this animate / refresh / wait?"
When a design choice conflicts with a rule here, the rule wins unless a
`docs/ARCHITECTURE_DECISIONS.md` entry explicitly overrides it and says why.

Stack is fixed (see `CLAUDE.md` §4): semantic server-rendered HTML, Askama
templates + fragments, Alpine.js CSP build, Alpine AJAX, modern CSS, minimal JS,
progressive enhancement. No React/Vue/Angular, no large component library.

---

## 1. The one governing principle

**Motion explains, never decorates. Structure earns its space, never fills it.**

Every animation and every pixel of layout answers to one test:

- Animation: *does removing it lose information or orientation?* If no → cut it.
- Layout region: *does this region do a job for this user on this task?* If no →
  cut it. "Doesn't waste space" does not mean cramped — it means every region
  present is pulling weight. Whitespace around a single critical alert is doing
  a job (drawing the eye); whitespace as filler is not.

This mirrors the backend rule "never add infrastructure to compensate for a bad
query." Here: never add motion or chrome to compensate for a weak layout.

## 2. Motion rules (hard)

- **Never animate content in on page load.** A student waiting for their own
  schedule to fade in is being made to wait to read data that was already in the
  HTML. Server-rendered content appears instantly, at rest.
- **Motion only touches things the user just triggered**, and only elements
  already on screen or entering because of that action: a menu opening, a dialog
  appearing, a row transitioning to a confirmed state, a drawer sliding in.
- **Exactly three animated surfaces exist. No fourth without an ADR:**
  1. Dropdown menus (primary nav, row action menus).
  2. Dialog / drawer (document requests, destructive confirmations).
  3. Mobile navigation sheet.
- **Durations come from tokens, never ad hoc:** `--dur-fast ≈ 120ms` for
  hover / focus / menu; `--dur-base ≈ 180ms` for dialog / drawer / sheet.
  Nothing on a critical interaction path exceeds ~200ms. One enter easing curve,
  one exit curve, both from tokens.
- **`prefers-reduced-motion: reduce` disables every transform/opacity transition**
  and keeps the instant state change. The app must be fully usable, and feel
  finished, with zero motion. Motion is enhancement, never load-bearing. This is
  a WCAG 2.2 AA requirement, not a nicety.
- No spinners for work that finishes in <200ms — show the result. A spinner is
  only honest when the wait is real (server round trip, document generation).

## 3. Optimistic UI is forbidden on truthful operations

Never paint a success state before the server confirms it, for:
registration, drop/add, grade entry, grade publication, document requests, or
document downloads. These show an explicit in-progress state
(`Checking…` / `Submitting…`) and then the **real server outcome**. A false
"Enrolled" on a section that was actually full is the single worst thing this
UI can do — it is both a UX failure and a correctness failure. Optimistic
updates are acceptable only for trivial, reversible, non-truthful UI state
(collapsing a panel, toggling a local filter).

## 4. The read-path / write-path split (mirrors the backend)

- **Read-path interactions are instant and local.** Catalog search, filtering,
  sorting, client-side within an already-loaded list → happen in-page with no
  round trip, no full reload. This is the frontend face of the backend's cached
  read path.
- **Write-path interactions wait for the truth.** Anything that mutates state
  goes to the server and reflects the committed outcome (see §3).

## 5. Alpine AJAX vs. full navigation — where the line is

- **Fragment swap (Alpine AJAX):** mutations that change one region —
  register / drop, in-page search results, grade entry, document request submit,
  approve/reject in a queue. The server returns the updated fragment; one region
  repaints; no full-page refresh.
- **Full server-rendered page nav:** moving between distinct areas (dashboard →
  registration → schedule → grades). Real URLs, real history, back button works.
- **The floor:** every fragment interaction must also work as a plain full-page
  form submit when JavaScript is absent. Alpine AJAX is enhancement over a
  working HTML form, never a replacement for one. If a flow breaks with JS off,
  it is not done.

## 6. Density adapts to role and task

Same design system, different density by who's working and what they're doing:

- **Student, single-decision screens** (dashboard, one hold, one deadline):
  calm, generous spacing, large touch targets, one clear primary action.
- **Registrar / records, scanning screens** (40 sections, rosters, queues):
  tight bordered table rows, not rounded cards; compact density; scannable
  columns; keyboard-first. Reuse the same tokens and components, shift density.
- Never make a scanning screen calm-and-airy (wastes an expert's time) or a
  single-decision screen dense (intimidates and buries the action).

## 7. Performance budgets (enforced, from `CLAUDE.md` §7 + brief §4)

- Useful HTML returned from the server immediately; first meaningful content does
  not wait on JS.
- No JS required to render navigation or basic page structure.
- Small compressed CSS/JS payloads; fingerprinted immutable production assets
  with long cache lifetimes.
- No large images on critical workflow pages.
- No N+1 browser requests for data that could arrive in the initial page or one
  fragment response.
- No layout shift from late-loading chrome — reserve space for anything that
  arrives after first paint.
- Responsive tables stay usable on small screens (see §6 density note).

## 8. Accessibility (WCAG 2.2 AA, testable extent)

- Keyboard navigation through every interactive element; visible focus states on
  all of them (functional focus ring is the one allowed box-shadow).
- Accessible labels on all controls; accessible, programmatically-associated
  validation and error messages.
- Semantic HTML first (`<nav> <main> <table> <button>`), ARIA only to fill gaps.
- Color never the sole signal — status carries an icon or text too (the blocked
  registration row names its reason; it is not just red).
- Automated a11y checks on the critical pages listed in `CLAUDE.md` completion
  criteria; a documented manual checklist for what automation can't catch.
- Useful empty states — an invitation, not an apology (per CDS content rules).

## 9. The design system (build once, reuse everywhere)

Tokens first (CSS custom properties): color roles, surfaces, borders, spacing
scale, radius, the two motion durations, the two easing curves. Then the
primitives, each built and accessibility-checked once: layout shells, form
controls, buttons, alerts/banners, cards, tables, dialog, drawer, nav (desktop +
mobile sheet), status badges, loading states, error states, empty states.
Document the system as built in `docs/FRONTEND_DESIGN_SYSTEM.md`. A screen may
not introduce a bespoke component when a primitive exists; extend the primitive
and note why.

## 10. Copy voice (from CDS content rules)

Sentence case everywhere. No terminal punctuation on labels/buttons/headings.
Verb-first buttons ("Register", "Request transcript", "Publish grades"). Errors
say what happened then what to do, no "Error:" prefix, never surface raw
exception strings. "Your schedule", never "My schedule". No "successfully",
no "please", no "!". Empty states name the space and invite an action.

## 11. What is explicitly rejected

Content-load animations. Count-up numbers. Decorative spinners. Hero images on
workflow pages. Full-page refreshes where a fragment swap fits. Optimistic
success on truthful operations (§3). A fourth animated surface (§2). Bespoke
one-off components duplicating a primitive (§9). Motion that survives
`prefers-reduced-motion: reduce`. Dense layouts on single-decision screens and
airy layouts on scanning screens (§6).
