// Reject relative Markdown links, and rustdoc intra-doc links, in site content.
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
// The directory form is the same defect without the `.md`: `[Architecture]
// (architecture/)` is emitted verbatim, resolves against whatever URL the
// reader happens to be on, and 404s from anywhere but the section index. So the
// rule is stated as an allow-list rather than a pattern of known-bad shapes —
// an internal target is `@/`-rooted or it is not a link this site can validate.
import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

const ROOT = new URL("../content/", import.meta.url).pathname;

/** Every Markdown link and image, whatever its target. */
const MD_LINK = /!?\[[^\]]*\]\(\s*([^)\s]+)(?:\s+"[^"]*")?\s*\)/g;

/**
 * Targets a Markdown link may carry:
 *
 * - `@/docs/section/page.md` — Zola's internal link syntax, which `zola check`
 *   resolves and fails the build on when it is dead. Section indexes are
 *   `@/docs/section/_index.md`, not `section/`.
 * - `http:` / `https:` — external, checked by `zola check --skip-external-links`
 *   only when that flag is absent, but never silently wrong internally.
 * - `#anchor` — same-page.
 * - `mailto:` — not a page.
 *
 * Anything else is relative, unresolved and unvalidated.
 */
const ALLOWED_TARGET = /^(?:@\/|https?:|#|mailto:)/;

async function* markdownFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* markdownFiles(path);
    else if (entry.name.endsWith(".md")) yield path;
  }
}

/**
 * A rustdoc intra-doc link in a file rustdoc never renders.
 *
 * `[`Thing`]` resolves inside a `///` comment and nowhere else. In Markdown it
 * is a *shortcut reference link*, so without a matching `[`Thing`]: target`
 * definition it renders as the literal text `[Thing]` — brackets and all.
 *
 * The habit comes from writing doc comments all day and it has landed three
 * separate ways in this repo: a private item linked from a public doc (which
 * `cargo doc` rejects), a link to a crate `xtask` does not depend on (same),
 * and twelve of these in READMEs and site pages, which nothing rejected because
 * Markdown has no opinion about them.
 */
const RUSTDOC_LINK = /\[(`[^`\]\n]+`)\](?![(:])/g;

/** Fenced and indented code, where these shapes are quotations rather than links. */
const stripCode = (src) =>
  src.replace(/```[\s\S]*?```/g, "").replace(/^ {4,}\S.*$/gm, "");

const offenders = [];
const rustdocLinks = [];
for await (const file of markdownFiles(ROOT)) {
  const src = await readFile(file, "utf8");
  const lines = src.split("\n");
  lines.forEach((line, i) => {
    for (const m of line.matchAll(MD_LINK)) {
      if (ALLOWED_TARGET.test(m[1])) continue;
      offenders.push(`${relative(ROOT, file)}:${i + 1}  ${m[0]}  → target "${m[1]}"`);
    }
  });

  const body = stripCode(src);
  const defined = new Set(
    [...body.matchAll(/^\[([^\]]+)\]:/gm)].map((m) => m[1]),
  );
  for (const m of body.matchAll(RUSTDOC_LINK)) {
    if (defined.has(m[1])) continue;
    rustdocLinks.push(`${relative(ROOT, file)}  [${m[1]}]`);
  }
}

if (rustdocLinks.length > 0) {
  console.error(
    `\ncheck-links: ${rustdocLinks.length} rustdoc intra-doc link(s) in Markdown.\n\n` +
      rustdocLinks.map((o) => `  ${o}`).join("\n") +
      `\n\nMarkdown has no intra-doc links: without a \`[\`Thing\`]: target\` definition\n` +
      `each renders as the literal text \`[Thing]\`. Drop the brackets and keep the\n` +
      `code span, or write a real link.\n`,
  );
  process.exit(1);
}

if (offenders.length > 0) {
  console.error(
    `\ncheck-links: ${offenders.length} relative Markdown link(s).\n\n` +
      offenders.map((o) => `  ${o}`).join("\n") +
      `\n\nZola resolves internal links written as \`@/docs/section/page.md\` and fails\n` +
      `the build when one is dead. Anything else — \`page.md\`, \`section/\`, \`../x\` —\n` +
      `is emitted verbatim, so it is never validated and 404s in the browser.\n` +
      `Rewrite each as an \`@/\` link; a section index is \`@/docs/section/_index.md\`.\n`,
  );
  process.exit(1);
}

console.log(
  "check-links: every internal link is @/-rooted and no rustdoc intra-doc link " +
    "is left in Markdown",
);
