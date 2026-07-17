# Performance

## Frontend budgets (Phase 7 — established and enforced by tests)

| Budget | Limit | Actual (2026-07-16) | Enforced by |
|---|---|---|---|
| Stylesheet size | ≤ 16 KiB uncompressed | ~5.5 KiB (~1.8 KiB gzipped) | `asset_sizes_stay_inside_the_budget` |
| Script size | ≤ 4 KiB uncompressed | ~1.2 KiB | same |
| Images on workflow pages | none | none | `templates_carry_no_images_or_csp_violations` |
| Third-party/external resources | none | none | same |
| Asset caching | fingerprinted URL + `public, max-age=31536000, immutable` | ✓ | `assets_serve_fingerprinted_with_an_immutable_cache_lifetime` |
| Compression | gzip (brotli/zstd also available) via `Compress` middleware | ✓ | `assets_compress_when_the_client_accepts_gzip` |

Consequences: a first page view costs the HTML plus one ~2 KiB stylesheet
and one ~1 KiB script; every later view costs the HTML alone (assets are
immutable-cached). There is no client-side data fetching, no layout shift
from late chrome (notices are server-rendered), and no blank-screen state
(PRG navigation keeps the previous page until the response arrives; the
submitting form shows a busy state via `aria-busy`).

## Backend benchmarks (Phase 8 — not yet run)

The three benchmark classes from CLAUDE.md §4 (in-process gates, read
paths, durable transactional paths) with hardware/dataset/concurrency/
p50-p95-p99 metadata land with plan item 8.4. Until then this file
records only the frontend budgets above; no backend performance numbers
have been measured or may be claimed.
