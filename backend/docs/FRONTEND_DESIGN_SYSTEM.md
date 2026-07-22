# Frontend design system — as built

Server-rendered Askama pages over the shared design system in `frontend/`
(ADR-12): one token sheet, one component stylesheet, one JS bundle (Alpine
CSP build + progressive enhancements). Every flow works with JavaScript
off — the backend test suite never executes scripts, so that is proven on
every run. FRONTEND.md is the constitution; this file is the inventory.

Live reference: `frontend/templates/pages/gallery.html` renders every
primitive once (`cd backend && cargo run -- render-pages <dir>`); the axe
harness (`cd frontend && npm test`) audits it.

## Files and serving

| Source | Bundled into | Budget (tested) |
|---|---|---|
| `frontend/styles/{tokens,base,components}.css` via `app.css` | `dist/app-<hash>.css` | ≤ 32 KiB uncompressed |
| `frontend/js/app.js` (imports `@alpinejs/csp`) | `dist/app-<hash>.js` | ≤ 80 KiB uncompressed (~61 KiB, ~19 KiB gzipped) |

`npm run build` fingerprints into `frontend/dist/` (committed); the
backend loads dist at startup and serves it with
`Cache-Control: public, max-age=31536000, immutable` plus gzip. A changed
bundle gets a new URL, so the year-long lifetime can never pin stale
chrome. Asset URLs are license-exempt so login and locked pages keep
their styling.

## Design tokens (`frontend/styles/tokens.css` — the single source of truth)

Extracted from the reference screens in `docs/design-references/`; light
and dark (via `prefers-color-scheme`). **Every text pair is
contrast-tested in both themes** (`design_tokens_meet_wcag_contrast`).

- Surfaces `--surface-0/1/2` (page < panel < card), text
  `--text-primary/secondary/muted`, borders `--border`, `--border-strong`.
- Four color roles, each a `--text-*`/`--bg-*`/`--border-*` triple:
  accent (blue), success (green), warning (amber), danger (red).
- `--action` + `--action-ink` — the primary button pair, fixed across
  themes (deviation from the references, noted in the file: their
  dark-mode button label sat at 2.1:1).
- Radius `--radius` (controls) / `--radius-card`; spacing scale
  `--space-1…6` (4/8/12/16/24/40 px).
- Motion: `--dur-fast` 120 ms (hover/focus/menu), `--dur-base` 180 ms
  (dialog/sheet); `--ease-enter`, `--ease-exit`. These four tokens are the
  entire motion vocabulary (FRONTEND.md §2).

## Primitives (`frontend/styles/components.css` + `frontend/templates/components/ui.html`)

Leaf components are Askama macros (`{% import "components/ui.html" as ui
%}`); composites that wrap arbitrary content are markup patterns — copy
them from the gallery. All are reduced-motion aware (global kill switch in
`base.css`) and focus-visible ringed (one global rule, the only permitted
box-shadow).

- **App shell** (`base.html`): desktop = persistent nav rail
  (`nav.rail`, 200 px grid column); ≤ 860 px = top bar with the mobile
  nav sheet — animated surface #3, a `<details class="nav-sheet">` so it
  opens/closes with JS off. Only one nav is ever in the accessibility
  tree. Sign-in renders the bare shell (`shell-bare` + empty `rail`
  block).
- **Buttons**: `button`/`.btn` default; `.btn-primary` (action tokens);
  `[disabled]` dims; in-progress = submit-once enhancement swaps in
  `data-busy-label` ("Checking…") and sets `form[aria-busy]` — honest
  wait, never optimistic success (FRONTEND.md §3). Macro:
  `ui::busy_button(label, busy_label, primary)`.
- **Form controls**: styled `input`/`select`/`textarea`; `.field` groups
  label + control + `.field-hint`/`.field-error` (associated via
  `aria-describedby`, flagged with `aria-invalid`). Macros: `ui::field`,
  `ui::search` (sr-only label + icon). Selects stay native elements.
- **Alert/banner**: `.alert` + `.alert-{accent,success,warning,danger}`;
  the macro assigns `role="alert"` to warning/danger and `role="status"`
  otherwise, so a styled alert is always an announced alert.
- **Card**: `.card` (surface-2, hairline border, card radius);
  `.card-flush` for full-bleed tables.
- **Data table**: calm by default (roomy rows); `.table-dense` for
  scanning screens (13 px, tight rows, row hover) — same markup, density
  shifts per FRONTEND.md §6. Always `caption` (visually hidden) +
  `scope="col"`; wrap in `.table-wrap` so narrow screens scroll the
  table, not the page. `.num` for tabular figures.
- **Dialog** — animated surface #2: native `<dialog>` (platform focus
  trap + Escape), opened by any `[data-dialog-open="id"]` button, closed
  by `[data-dialog-close]` (both wired in `app.js`), `aria-labelledby`
  its `h2`. Destructive confirmations put the real POST form inside.
- **Dropdown menu** — animated surface #1: `<details class="menu">` +
  `.menu-panel` of links/buttons; native disclosure semantics, works JS
  off.
- **Status badge**: `.badge` + modifier from the ONE Rust-side map
  (`shared::assets::badge_class`) so a status never renders in two
  colors. Macro: `ui::badge(status)`.
- **Loading state**: `ui::loading(message)` — `role="status"`, spinner +
  text; only for real waits (server round trip, document generation).
  Under reduced motion the spinner freezes and the text carries it.
- **Error state**: `ui::error_state(what_happened, what_to_do)` —
  `role="alert"`, no "Error:" prefix, never a raw exception string.
- **Empty state**: `ui::empty_state(message, href, action_label)` — names
  the space and invites an action, not an apology.
- **Registration row** (`components/section_row.html`, session 1): one
  `<tr>` fragment shared by the registration page (included per row) and
  the fragment endpoints (rendered alone, swapped in after register/drop),
  so a swapped row is byte-identical to a fresh page load. Four states via
  `RegistrationRow` helpers (available / enrolled / low-seats / blocked —
  a blocked row names its reason in `.reason`, never just a color). The
  page-level `.meta` line + `data-search` attributes feed the instant
  filter.
- **Dashboard strip** (`.strip`): compact label/value list rows inside a
  card (schedule strip, campus events).
- **Weekly schedule** (`.week`/`.day`): day sections with time-sorted
  lists; stacked mobile-first, grid columns (5, or 7 with `.week-7`) from
  861 px — one structure, CSS-only, nothing duplicated.
- **Grade-entry roster** (session 2, instructor-grades reference): context
  header (`.crumb`, `.ctx`) with the section switcher — the dropdown-menu
  primitive extended with `.menu-sub` (group label) and `.cur` (current
  item), listing only the viewer's assignments; window banner via the
  alert primitive; `.prog` progress line; rows in three states — muted
  "Not entered", amber Draft (`select.grade.dirty` + amber badge), green
  Published (`tr.published`, `.grade-final`, read-only); per-row Save with
  busy label; inline row errors (`.field-error`, aria-describedby);
  `.actions` sticky bar whose publish form is confirm-gated
  (`data-confirm-dialog`, see JavaScript policy) with `.warnbox` stating
  the consequence inside the dialog.
- **Request drawer** (`details.drawer` + `.drawer-panel`, session 3):
  animated surface #2 on native `<details>` — the document-request form
  opens and submits with JS off; enter-down animation on open only.
- **Status rows** (`components/document_row.html` student side,
  `document_status_row.html` officer side): one `<tr>` per document
  request carrying `data-status` and, in non-terminal states, `data-poll`
  pointing at its owner-/officer-scoped row-fragment endpoint. The
  polling enhancement (see JavaScript policy) swaps in the
  server-rendered row, so a status on screen is always the real backend
  row; terminal rows drop `data-poll` and polling stops itself. Status is
  always badge + the status word from the ONE badge map — never color
  alone.
- **Registrar shell** (`ui::registrar_nav(current)`, session 4): the same
  topbar + rail structure as the default shell with the registrar's own
  sections (Overview, Terms & windows, Sections, Courses, Students,
  Overrides); `current` sets `aria-current="page"`, styled once on
  `.navlink`. Registrar pages override the `rail` block with it and use
  `.wrap-wide` (1200 px) — scanning screens opt out of the calm column.
- **Metric tiles** (`.tiles`/`.tile`, registrar-dashboard reference): the
  at-a-glance strip — label `.k`, value `.v`, hint `.h`; `.tile-warn` for
  the needs-attention tile when its count is non-zero. Two columns on
  mobile.
- **Panel + worklist + windows** (`.panel`, `.queue`, `.win`): bordered
  card with a heading row (`.count` pill for the worklist size); queue
  items carry a severity dot (`.dot-danger`/`.dot-warn`) plus title and
  detail text — the reason is always written out, never the dot alone.
  Window rows pair label/range with a state badge (open/upcoming/closed
  from the ONE badge map).
- **Sortable dense table** (`.sortbtn`, session 4): a scanning table's
  sortable column headers are real buttons inside `th` (keyboard-first);
  the enhancement (see JavaScript policy) reorders loaded rows in place
  and sets `aria-sort` (arrow drawn via CSS). With JS off the server's
  order stands. Combined with the instant filter via `tr[data-search]`
  and the `.tabletop` header row (title + `.termpick` + search).
- **Inline management forms** (`.inline-form`, `.detail-card`): label +
  control + busy button on one line inside a card — the shape of every
  section-detail and student-detail mutation (capacity, meetings,
  instructor, standing, holds). Errors re-render the page with the alert
  primitive and an honest 409/422; successes are PRG redirects with a
  fixed notice string.
- **Unofficial document** (`.print-doc`, `.doc-head`, `.doc-actions`,
  `.doc-foot`): identity `<dl>` header + warning alert marking the page
  unofficial; `@media print` drops the chrome (`.rail`, `.topbar`,
  `.no-print`) so the page itself is the printable document — no PDF
  pipeline for unofficial output. `[data-print]` buttons call
  `window.print()` (CSP forbids inline handlers).

## Motion policy (enforced shape)

Three animated surfaces (menu, dialog, mobile sheet), enter-only
keyframes `enter-down`/`enter-up`, durations and curves only from tokens,
nothing animates on page load, and `prefers-reduced-motion: reduce`
disables every animation and transition globally. A fourth animated
surface requires an ADR (FRONTEND.md §2).

## JavaScript policy

`frontend/js/app.js` bundles Alpine's CSP build (CSP stays
`script-src 'self'`, no inline anything) and the first-party
enhancements: submit-once/busy-label, bfcache restore, dialog wiring,
print buttons (`[data-print]`), the registration screen's instant search
filter (`[data-search-form]` over `tr[data-search]`, read path — zero
round trips), and the registration fragment swap: a `[data-fragment]`
form POSTs with an `X-Fragment` header and the server answers with the
single re-rendered row in its committed state (200, or 409 with the
named denial), which replaces the old `<tr>` — an honest "Checking…"
wait, never an optimistic success. `[data-confirm]` drop forms route
through the shared `#drop-dialog` first. Document status rows carrying
`data-poll` re-fetch their server-rendered fragment every 4 s and swap it
in (honest pipeline view — approved → generating → ready only as the
worker actually completes; terminal rows stop polling by omitting the
attribute). Sortable scanning tables (registrar): a click on a
`th button[data-sort]` reorders the already-loaded `tr[data-search]`
rows by that column, numeric-aware, and sets `aria-sort` — read path,
zero round trips, server order with JS off. Alpine AJAX fragment swaps
are enhancement over working HTML forms, never a replacement
(FRONTEND.md §5). Server-side idempotency keys remain the real
duplicate guarantee — the script is UX, not correctness.

## Automated checks

- Backend suite (every CI run): `assert_page_a11y` inside every UI flow
  test (lang, viewport, title, skip link, main, one h1, label per
  control, caption + scopes, no inline style/handlers);
  `design_tokens_meet_wcag_contrast` (light + dark);
  `asset_sizes_stay_inside_the_budget`;
  `templates_carry_no_images_or_csp_violations` (pages + components);
  fingerprint/immutable/gzip serving tests.
- Frontend step (`npm test`): renders the critical pages via
  `cargo run -- render-pages`, then runs axe-core over each (jsdom;
  layout-dependent rules like rendered contrast stay manual, below).
- CI rebuilds dist and diffs it against the commit, so the served assets
  always match the sources.

## Manual accessibility checklist (what automation can't catch)

Run before a release that changed templates or CSS; record date +
findings in the commit:

1. **Keyboard walk** of each role journey (student register/drop,
   instructor grade entry, officer publish + document queue, admin
   calendar, platform license panel): every control reachable in a
   sensible order, visible ring everywhere, no traps, skip link lands on
   main, menu/dialog/sheet close on Escape and return focus.
2. **Screen reader pass** (NVDA/VoiceOver): titles announced, form errors
   announced on re-render, table headers read with cells, badges read as
   text, the nav sheet announces expanded/collapsed.
3. **200 % zoom and 320 px width**: no horizontal scroll except inside
   `.table-wrap`, nothing clipped; the mobile sheet is reachable.
4. **Color-independence**: statuses distinguishable by text alone;
   blocked rows name their reason, never just red.
5. **prefers-reduced-motion**: menu, dialog, and sheet appear instantly;
   nothing else moves; the app feels finished, not broken.
6. **Real browser + axe** on the critical pages: jsdom cannot compute
   rendered contrast, sticky-region overlap, or zoom reflow.
7. **Print** the student history page: chrome hidden, content legible.

## Adding a screen

Extend `pages/base.html` (or override `rail`/`shell_class` for bare
pages); compose primitives from this file — a screen may not introduce a
bespoke component when a primitive exists (extend the primitive and note
why here). Choose density by task (§6): calm for single decisions, dense
for scanning. Forms carry `csrf_token` (+ idempotency key if the POST
creates something); redirect on success, re-render inline with the honest
status code on denial. Then: fetch the page in a UI test and call
`assert_page_a11y`, and add it to `render-pages` so axe covers it.
