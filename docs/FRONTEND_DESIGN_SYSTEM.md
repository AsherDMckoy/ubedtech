# Frontend design system — as built (Phase 7)

One stylesheet, one small script, server-rendered Askama pages, plain HTML
forms with POST-redirect-GET. No framework, no build step, no third-party
code (ADR-11). Every page works with JavaScript disabled — the test suite
never executes scripts, so this is proven on every run.

## Files

| File | Served as | Budget (tested) |
|---|---|---|
| `web/assets/app.css` | `/assets/app-<sha256[..8]>.css` | ≤ 16 KiB uncompressed (currently ~5.5 KiB) |
| `web/assets/app.js` | `/assets/app-<sha256[..8]>.js` | ≤ 4 KiB uncompressed (currently ~1.2 KiB) |

Both are embedded in the binary (`shared/assets.rs`), fingerprinted from
their content at startup, and served with
`Cache-Control: public, max-age=31536000, immutable` — a changed file gets
a new URL, so the year-long lifetime can never pin stale chrome. Responses
are gzip-compressed (`Compress` middleware, tested). Asset URLs are
license-exempt so the login and locked pages keep their styling.

## Design tokens (CSS custom properties)

Defined at the top of `app.css`; **every text pair is contrast-tested**
(`design_tokens_meet_wcag_contrast` computes WCAG ratios from the real
stylesheet — change a color and the test says whether it still reads).

- `--ink` / `--ink-soft` on `--paper` / `--paper-dim` — body text (AA+).
- `--action` + `--action-ink` — buttons and the header bar.
- `--link` — anchors.
- Status pairs (`--ok-*`, `--warn-*`, `--bad-*`, `--info-*`, `--mute-*`) —
  dark text on light fields, used by badges and alerts.
- `--focus` — the shared focus ring (≥ 3:1 against both paper tones).

## Components (all plain HTML + CSS; classes only where semantics fall short)

- **Layout**: `body > header` (brand bar + primary nav), centered `main`
  (64rem max), `.skip-link` appears on focus.
- **Forms**: bare `label`/`input`/`select` elements are styled directly;
  labels either wrap their control or use `for=`. Validation problems
  re-render the page with the message in a `role="alert"` region and an
  honest status code (401/409/422); successes redirect (PRG).
- **Buttons**: `button` styled directly; `[disabled]` dims with a progress
  cursor.
- **Alerts**: `[role="status"]` (info) and `[role="alert"]` (problem) —
  the roles ARE the styling hook, so a styled alert is always an announced
  alert. Server-rendered, so there is no late-arriving chrome and no
  layout shift.
- **Cards**: `article` (used by the officer queue).
- **Tables**: styled directly; every table carries a `caption` and
  `scope="col"` headers (audit-enforced). Under 40rem viewports tables
  scroll horizontally instead of breaking the layout.
- **Status badges**: `.badge` + modifier from ONE Rust-side map
  (`shared::assets::badge_class`) so the same status never renders in two
  colors on two pages.
- **Loading state**: submitting a form sets `aria-busy` + disables its
  buttons (app.js); CSS dims the form. On slow networks the user sees the
  dimmed form and progress cursor instead of a dead page; without JS the
  browser's own navigation feedback applies.
- **Dialogs**: none exist — no page needs one. Use native `<dialog>` when
  one genuinely does; do not add a dialog component before then.

## JavaScript policy (ADR-11)

`app.js` is progressive enhancement only, and only one behavior: block
double submission and show the busy state. Server-side idempotency keys
(enrollment, document requests) remain the real duplicate guarantee — the
script is UX, not correctness. Interactions are full-page PRG on purpose:
pages are a few KiB of HTML plus one immutably-cached stylesheet, so a
fragment-swap layer (the design docs sketched Alpine AJAX) would add a
vendored dependency, CSP surface, and a second rendering path without a
measurable win. Revisit only with a profile showing PRG navigation is a
real user-facing cost.

## Automated checks (run in CI with the ordinary test suite)

- `assert_page_a11y` runs against every critical page inside the existing
  UI flow tests — login, catalog, registration, grades, history,
  instructor sections, roster, documents, officer queue, admin calendar,
  license panel. It enforces: `lang`, viewport meta, title, skip link,
  `main` landmark, exactly one `h1`, a label per visible control, caption
  + column scopes per table, and no inline styles/handlers (CSP).
- `design_tokens_meet_wcag_contrast` — WCAG AA ratios from the stylesheet.
- `asset_sizes_stay_inside_the_budget` — CSS/JS byte caps.
- `templates_carry_no_images_or_csp_violations` — no `<img>`, no inline
  style/handlers, no external URLs, every page extends the shared base.
- `assets_serve_fingerprinted_with_an_immutable_cache_lifetime`,
  `assets_compress_when_the_client_accepts_gzip`.

## Manual accessibility checklist (what automation can't catch)

Run before a release that changed templates or CSS; record date + findings
in the PR/commit:

1. **Keyboard walk** of each role journey (student register/drop,
   instructor grade entry, officer publish + document queue, admin
   calendar, platform license panel): every control reachable in a sensible
   order, visible focus ring everywhere, no traps, skip link lands on main.
2. **Screen reader pass** (NVDA/VoiceOver): page titles announced, form
   errors announced when the page re-renders (alert regions), table
   headers read with cells, badge statuses read as text.
3. **200 % zoom and 320 px width**: no horizontal scroll except inside
   tables, nothing clipped or overlapped.
4. **Color-independence**: statuses distinguishable with badges' text
   alone (they always carry the status word, never color-only).
5. **prefers-reduced-motion**: nothing animates today; re-check if any
   animation is ever added.
6. **Real browser + axe** (or Lighthouse a11y) on the eleven critical
   pages: the structural audit is string-level and cannot compute
   accessible names, contrast of rendered states, or ARIA misuse.
7. **Print** the student history page: chrome hidden, content legible.

## Adding a page

Extend `pages/base.html`; use semantic elements first and existing classes
(`.badge`, `.skip-link`) second; render notices via `role="status"` /
`role="alert"`; forms carry `csrf_token` (and an idempotency key if the
POST creates something); redirect on success, re-render inline on denial
with the honest status code. Fetch the page in a UI test and call
`crate::shared::assets::assert_page_a11y` on the body — the audit and the
budgets then hold for your page forever.
