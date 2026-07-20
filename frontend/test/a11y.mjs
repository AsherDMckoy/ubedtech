// Accessibility harness: axe-core over the server-rendered critical pages
// (written by `cargo run -- render-pages`, see package.json pretest).
// jsdom has no layout engine, so layout-dependent rules (color-contrast)
// are excluded here — contrast is proven from the design tokens by the
// backend test `design_tokens_meet_wcag_contrast`, and the rendered-page
// pass lives in the manual checklist (docs/FRONTEND_DESIGN_SYSTEM.md).
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { JSDOM } from "jsdom";

const require = createRequire(import.meta.url);
const axeSource = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");

const dir = process.argv[2] ?? "test/pages";
const pages = readdirSync(dir).filter((name) => name.endsWith(".html"));
if (pages.length === 0) {
  console.error(`no rendered pages in ${dir} — run \`npm test\` (pretest renders them)`);
  process.exit(1);
}

let failures = 0;
for (const name of pages.sort()) {
  const html = readFileSync(`${dir}/${name}`, "utf8");
  const { window } = new JSDOM(html, { runScripts: "outside-only" });
  window.eval(axeSource);
  const results = await window.axe.run(window.document.documentElement, {
    rules: { "color-contrast": { enabled: false } },
  });
  if (results.violations.length === 0) {
    console.log(`ok   ${name} (${results.passes.length} rules passed)`);
    continue;
  }
  failures += results.violations.length;
  console.error(`FAIL ${name}`);
  for (const violation of results.violations) {
    console.error(`  [${violation.impact}] ${violation.id}: ${violation.help}`);
    for (const node of violation.nodes) {
      console.error(`    ${node.html}`);
    }
  }
}

process.exit(failures === 0 ? 0 : 1);
