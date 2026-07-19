# frontend/

Everything a designer touches: Askama templates, the design-token sheet and
component CSS, first-party JS, and the asset pipeline. The backend renders
the templates and serves `dist/` — it never runs Node (ADR-12).

## Layout

- `templates/` — Askama templates (`backend/askama.toml` points here).
  `templates/pages/` are full pages extending `pages/base.html`;
  `templates/components/` are reusable macros.
- `styles/` — `app.css` is the entry; it imports `tokens.css` (the single
  source of truth for color, spacing, radius, motion — see FRONTEND.md §9),
  `base.css`, and `components.css`.
- `js/app.js` — the one JS entry: Alpine CSP build + progressive
  enhancements (submit-once, dialog wiring). Every flow works with JS off.
- `dist/` — built, fingerprinted output. **Committed**, so backend builds
  and tests need no Node. Never edit by hand; never edit `styles/` or
  `js/` without rebuilding.
- `test/` — the accessibility harness (axe over rendered pages).

## Commands

Requires Node ≥ 26 (`.nvmrc`). All commands run in `frontend/`.

```sh
npm ci          # install exact pinned dependencies (lockfile committed)
npm run build   # bundle + minify + fingerprint into dist/ — commit the result
npm test        # a11y harness: renders critical pages, runs axe over them
```

Dev loop: edit `styles/` / `js/`, `npm run build`, restart the backend
(it loads `dist/` at startup). There is no watch mode until someone misses
one — the build is ~15 ms.

Production: `npm ci && npm run build` at release-preparation time; deploy
the repo state. The backend serves `dist/` from
`APP_FRONTEND_DIST` (default: `../frontend/dist` relative to the backend
crate) with immutable cache headers.
