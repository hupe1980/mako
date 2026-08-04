// Validate every Mermaid diagram on the site, and the script that renders them.
//
//   node site/tools/check-mermaid.mjs
//
// Two failures this catches, both of which shipped silently before it existed:
//
//   1. A diagram whose syntax Mermaid rejects. Nothing in `zola build` or
//      `zola check` parses diagram source, so a broken one renders as an error
//      box — or as nothing — on a published page.
//
//   2. `mermaid-init.js` regressing to `startOnLoad` without an explicit
//      `run()`. Mermaid registers its auto-run on `window.load` at *module
//      evaluation* time; the site imports it dynamically from a CDN, so on a
//      cold cache the module evaluates after `load` has already fired and the
//      diagrams never render. It works on a warm cache, which is what makes the
//      bug so easy to reintroduce.
//
// Requires `npm install mermaid@11 jsdom` (see the site workflow).

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { JSDOM } from "jsdom";

const ROOT = new URL("../..", import.meta.url).pathname;
const CONTENT = join(ROOT, "site/content");
const TEMPLATES = join(ROOT, "site/templates");
const INIT = join(ROOT, "site/static/js/mermaid-init.js");

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

/** Every diagram on the site, as `{file, line, src}`. */
function diagrams() {
  const found = [];
  const push = (file, src, body, offset) => {
    found.push({
      file: relative(ROOT, file),
      line: src.slice(0, offset).split("\n").length,
      src: body,
    });
  };
  for (const file of walk(CONTENT).filter((f) => f.endsWith(".md"))) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
      push(file, src, m[1], m.index);
    }
  }
  // The landing page embeds `<pre class="mermaid">` directly.
  for (const file of walk(TEMPLATES).filter((f) => f.endsWith(".html"))) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(/<pre class="mermaid">\n([\s\S]*?)<\/pre>/g)) {
      push(file, src, m[1], m.index);
    }
  }
  return found;
}

let failures = 0;

// ── 1. The renderer must not depend on `startOnLoad` ─────────────────────────
{
  const init = readFileSync(INIT, "utf8");
  const startsOnLoad = /startOnLoad:\s*true/.test(init);
  const callsRun = /mermaid\.run\s*\(/.test(init);
  if (startsOnLoad || !callsRun) {
    failures++;
    console.error(
      "FAIL site/static/js/mermaid-init.js: diagrams must be rendered by an " +
        "explicit `mermaid.run()`, not `startOnLoad`.\n" +
        "      Mermaid attaches its auto-run to `window.load` when the module " +
        "evaluates. This module is imported dynamically from a CDN, so on a " +
        "cold cache it evaluates after `load` has fired and nothing renders.\n" +
        `      startOnLoad:true present=${startsOnLoad}  mermaid.run() called=${callsRun}`,
    );
  }
}

// ── 2. Every diagram must parse ───────────────────────────────────────────────
const all = diagrams();
if (all.length === 0) {
  failures++;
  console.error("FAIL: no diagrams found — the extractor is probably broken.");
}

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  pretendToBeVisual: true,
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
Object.defineProperty(globalThis, "navigator", {
  value: dom.window.navigator,
  configurable: true,
});

const mermaid = (await import("mermaid")).default;
mermaid.initialize({ startOnLoad: false, securityLevel: "strict" });

for (const d of all) {
  try {
    await mermaid.parse(d.src);
  } catch (e) {
    failures++;
    const msg = String(e?.message ?? e).split("\n").slice(0, 3).join(" ");
    console.error(`FAIL ${d.file}:${d.line}  ${msg}`);
  }
}

console.log(
  `checked ${all.length} diagram(s) — ${failures === 0 ? "all valid" : `${failures} problem(s)`}`,
);
process.exit(failures === 0 ? 0 : 1);
