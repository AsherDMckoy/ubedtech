// Production asset build: bundle + minify the one JS entry (Alpine CSP
// build + our enhancements) and the one CSS entry, content-fingerprint the
// output names, write to dist/. dist/ is committed so the backend build and
// tests never invoke Node (ADR-12).
import { build } from "esbuild";
import { readdirSync, rmSync, writeFileSync } from "node:fs";

rmSync("dist", { recursive: true, force: true });

// Generated manifest of the sign-in photos (ADR-18): every jpg in
// images/, as fingerprinted URLs the login arrows page through. A photo
// is only fetched when shown. Regenerated on every build — gitignored.
const jpgs = readdirSync("images")
  .filter((name) => name.endsWith(".jpg"))
  .sort();
writeFileSync(
  "js/photos.gen.js",
  jpgs.map((name, i) => `import p${i} from "../images/${name}";`).join("\n") +
    `\nexport default [${jpgs.map((_, i) => `p${i}`).join(",")}];\n`,
);

await build({
  entryPoints: ["js/app.js", "styles/app.css"],
  bundle: true,
  minify: true,
  outdir: "dist",
  entryNames: "[name]-[hash]",
  assetNames: "[name]-[hash]",
  loader: { ".woff2": "file", ".jpg": "file" },
  logLevel: "info",
});

for (const name of readdirSync("dist").sort()) {
  console.log(`dist/${name}`);
}
