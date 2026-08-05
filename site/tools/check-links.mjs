// Reject bare relative Markdown links in site content.
//
// `zola check` runs with `internal_level = "error"`, so a dead `@/docs/…` link
// fails the build. That check only applies to Zola's own link syntax: a plain
// `[text](makod.md)` is passed through to the rendered HTML untouched and never
// resolved, so it neither fails the build nor works in the browser.
//
// That gap was not hypothetical. The docs were reorganised from a flat `docs/`
// directory into sectioned `site/content/docs/<section>/`, and 21 links kept
// pointing at the old flat neighbours — `[docs/makod.md](makod.md)` from
// `architecture/` resolving to `architecture/makod.md`, which does not exist.
// Every one rendered as a link and 404'd, and the site check reported success
// for years of builds.
//
// The rule: internal links use `@/docs/section/page.md`. This script fails on
// anything else so the class cannot come back.
import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

const ROOT = new URL("../content/", import.meta.url).pathname;

/** Markdown links whose target is a bare `*.md` path. */
const BARE_MD_LINK = /\[[^\]]*\]\((?!@\/|https?:|#)([^)\s]*\.md(?:#[^)\s]*)?)\)/g;

async function* markdownFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* markdownFiles(path);
    else if (entry.name.endsWith(".md")) yield path;
  }
}

const offenders = [];
for await (const file of markdownFiles(ROOT)) {
  const src = await readFile(file, "utf8");
  const lines = src.split("\n");
  lines.forEach((line, i) => {
    for (const m of line.matchAll(BARE_MD_LINK)) {
      offenders.push(`${relative(ROOT, file)}:${i + 1}  ${m[0]}  → target "${m[1]}"`);
    }
  });
}

if (offenders.length > 0) {
  console.error(
    `\ncheck-links: ${offenders.length} bare relative Markdown link(s).\n\n` +
      offenders.map((o) => `  ${o}`).join("\n") +
      `\n\nZola resolves internal links written as \`@/docs/section/page.md\` and fails\n` +
      `the build when one is dead. A bare \`page.md\` is emitted verbatim, so it is\n` +
      `never validated and 404s in the browser. Rewrite each as an \`@/\` link.\n`,
  );
  process.exit(1);
}

console.log("check-links: no bare relative Markdown links");
