// Holds the production bundle to a budget, gzip-compressed as it travels:
// `npm run check` builds the Desk and fails here when the initial script or
// the stylesheet outgrows it. A new dependency or a view imported eagerly
// shows up as a number in the pull request instead of as a slower first
// paint nobody measured. Raise a budget only with the reason beside it.
//
//   node scripts/umfang.ts            check dist/ (after `vite build`)

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { gzipSync } from "node:zlib";

const dist = resolve(import.meta.dirname, "../dist");

/**
 * gzip bytes of what the first paint loads: the entry script with every chunk
 * index.html preloads beside it, and the stylesheet. A view split off behind
 * `lazy()` costs nothing here until it is opened.
 */
const BUDGET: Record<"js" | "css", number> = {
  js: 250 * 1024,
  css: 12 * 1024,
};

const html = existsSync(resolve(dist, "index.html"))
  ? readFileSync(resolve(dist, "index.html"), "utf8")
  : "";
const files = [...html.matchAll(/(?:src|href)="\/(assets\/[^"]+\.(?:js|css))"/g)].map(
  (m) => m[1] ?? "",
);
const used = { js: 0, css: 0 };
for (const f of files) {
  const kind = f.endsWith(".css") ? "css" : "js";
  used[kind] += gzipSync(readFileSync(resolve(dist, f))).length;
}

let over = 0;
for (const kind of ["js", "css"] as const) {
  const kb = (n: number) => `${(n / 1024).toFixed(1)} KB`;
  const line = `umfang: ${kind} ${kb(used[kind])} of ${kb(BUDGET[kind])} gzip`;
  if (used[kind] > BUDGET[kind]) {
    console.error(`${line} — over budget`);
    over++;
  } else {
    process.stdout.write(`${line}\n`);
  }
}
if (files.length === 0) {
  console.error("umfang: no dist/index.html — run `vite build` first");
  over++;
}
process.exit(over ? 1 : 0);
