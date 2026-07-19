// Production asset build: bundle + minify the one JS entry (Alpine CSP
// build + our enhancements) and the one CSS entry, content-fingerprint the
// output names, write to dist/. dist/ is committed so the backend build and
// tests never invoke Node (ADR-12).
import { build } from "esbuild";
import { readdirSync, rmSync } from "node:fs";

rmSync("dist", { recursive: true, force: true });

await build({
  entryPoints: ["js/app.js", "styles/app.css"],
  bundle: true,
  minify: true,
  outdir: "dist",
  entryNames: "[name]-[hash]",
  logLevel: "info",
});

for (const name of readdirSync("dist").sort()) {
  console.log(`dist/${name}`);
}
