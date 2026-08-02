# Frontend Design Constitution — University Education Platform

The frontend equivalent of `CLAUDE.md`. These rules decide the arguments in
advance so no screen has to relitigate "should this animate / refresh / wait?"
When a design choice conflicts with a rule here, the rule wins unless a
`docs/ARCHITECTURE_DECISIONS.md` entry explicitly overrides it and says why.

Stack is fixed (see `CLAUDE.md` §4): semantic server-rendered HTML, Askama
templates + fragments, Alpine.js CSP build, Alpine AJAX, modern CSS, minimal JS,
progressive enhancement. No React/Vue/Angular, no large component library.

---

## 1. The one governing principle (amended by ADR-17)

**Motion explains and delights — but never lies, never blocks, never busts a
budget. Structure earns its space; polish is a job structure can do.**

Every animation and every pixel of layout answers to one test:

- Animation: *does it orient the user, or make the product feel finished?*
  Either is a valid job. What it may never do: delay reading data the server
  already sent, precede server truth (§3), survive `prefers-reduced-motion`,
  or exceed the motion vocabulary in §2.
- Layout region: *does this region do a job for this user on this task?*
  On first-impression surfaces (sign-in, dashboard, empty states), "selling
  the product" is a legitimate job — gradient, elevation, and CSS/SVG art may
  hold a region. On scanning screens it is not; density still wins there (§6).

This still mirrors the backend rule "never add infrastructure to compensate
for a bad query": polish decorates a layout that already works — it is never
a bandage over a weak one.

## 2. Motion rules (hard — vocabulary form, ADR-17)

The per-surface cap is replaced by a **closed vocabulary of five motion
classes**. Anything outside the vocabulary still needs an ADR.

1. **Structural chrome** (the original three surfaces + icon rail, ADR-16):
   menus, dialog/drawer, mobile sheet, rail collapse — `--dur-base`.
2. **Entrance choreography** (page load, one-time): card/tile/row groups may
   stagger in with transform+opacity. Hard limits: ≤ 6 stagger steps,
   ~40 ms apart, whole choreography at rest ≤ 400 ms, initial offset small
   (≤ 8 px). The data is in the HTML from byte one — the entrance is paint
   decoration on content that is already there, never a skeleton standing in
   for content the server already sent, and never on dense scanning tables.
3. **Micro-interactions**: hover lift on cards/buttons (small translateY +
   shadow-token step), pressed states, focus transitions — `--dur-fast`.
4. **State celebration**: *after* the server confirms a truthful operation,
   the confirmed row/badge may flourish (checkmark draw, badge pop,
   `--dur-slow`). Celebration decorates the truth; it never precedes it (§3).
5. **Count-up numbers on metric tiles**: sanctioned only as enhancement —
   the real final value is server-rendered in the HTML, the count-up plays
   over it. Reduced motion (or JS off) shows the value instantly.

Cross-cutting hard rules, unchanged in force:

- **Durations and easing come from tokens, never ad hoc:** `--dur-fast ≈
  120ms`, `--dur-base ≈ 180ms`, and (new) `--dur-slow ≈ 350ms` for entrance
  and celebration only. Nothing on a critical interaction path exceeds
  ~200ms — entrances and celebrations run beside interaction, never in
  front of it (a mid-entrance element is already clickable).
- **`prefers-reduced-motion: reduce` disables every class above** and keeps
  the instant state change. The app must be fully usable, and feel finished,
  with zero motion. WCAG 2.2 AA, not a nicety.
- No spinners for work that finishes in <200ms — show the result. A spinner
  is only honest when the wait is real (server round trip, document
  generation).

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
  with long cache lifetimes. The polish pass may raise the tested CSS budget
  (32 → 48 KiB uncompressed) in the same commit that spends it — budgets are
  renegotiated in the open, never silently exceeded.
- Eye candy is paid for in CSS and inline SVG — gradients, shadows,
  transforms, vector art. Never raster hero images on workflow pages, never a
  new dependency, never an animation library.
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

Optimistic success on truthful operations (§3). Motion that survives
`prefers-reduced-motion: reduce`. Motion outside the §2 vocabulary without an
ADR. Skeleton screens standing in for content the server already rendered.
Decorative spinners. Raster hero images on workflow pages (CSS/SVG art is the
sanctioned medium). Animation libraries and any new frontend dependency.
Parallax, scroll-jacking, autoplay media. Full-page refreshes where a
fragment swap fits. Bespoke one-off components duplicating a primitive (§9).
Dense layouts on single-decision screens and airy layouts on scanning
screens (§6).

*Moved from rejected to sanctioned-with-rules by ADR-17:* entrance
choreography (§2.2), count-up numbers (§2.5), state celebrations (§2.4).
