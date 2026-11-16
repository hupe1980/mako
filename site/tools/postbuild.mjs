// Post-build step for the docs site: run after `zola build`, over `public/`.
//
//   node site/tools/postbuild.mjs
//
// Writes, from the content tree and the built HTML:
//
//   public/<path>/index.md   a Markdown twin of every content page — the source
//                            minus front matter, headed `# <title>` and its URL,
//                            `@/…` links resolved to absolute site URLs
//   public/llms.txt          the llmstxt.org index: every twin with its description
//   public/llms-full.txt     every twin, concatenated
//   public/pagefind/         the Pagefind search index `static/js/search.js` reads
//
// Every list is derived from `content/`, never written by hand, so a page added
// or renamed cannot fall out of one. The run fails if a content page has no
// built HTML (the URL derivation and Zola disagree) or if Pagefind indexed a
// different number of pages than the content tree holds (a template lost its
// `data-pagefind-body`, and the page silently dropped out of search).

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import * as pagefind from "pagefind";

const SITE = new URL("..", import.meta.url).pathname;
const CONTENT = join(SITE, "content");
const PUBLIC = join(SITE, "public");

// The groups `llms.txt` lists, in order, keyed by the directory under `docs/`.
// The docs root itself is listed with the guide, where a reader starts.
const GROUPS = [
  ["guide", "Guide"],
  ["architecture", "Architecture"],
  ["reference", "Reference"],
  ["services", "Services"],
  ["regulatory", "Regulatory"],
  ["compliance", "Compliance"],
];

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (name.endsWith(".md")) out.push(p);
  }
  return out;
}

/** The top-level `key = "value"` pairs of a TOML front matter block. */
function frontMatter(src, file) {
  const m = src.match(/^\+\+\+\r?\n([\s\S]*?)\r?\n\+\+\+\r?\n?/);
  if (!m) throw new Error(`${file}: no +++ front matter`);
  const fm = {};
  for (const line of m[1].split(/\r?\n/)) {
    if (/^\s*\[/.test(line)) break; // a [table] ends the top-level keys
    const kv = line.match(/^([A-Za-z_][\w-]*)\s*=\s*(.+?)\s*$/);
    if (!kv) continue;
    const [, key, raw] = kv;
    if (raw.startsWith('"')) fm[key] = JSON.parse(raw);
    else if (raw.startsWith("'")) fm[key] = raw.slice(1, -1);
    else if (/^-?\d+$/.test(raw)) fm[key] = Number(raw);
    else fm[key] = raw;
  }
  return { fm, body: src.slice(m[0].length) };
}

/**
 * The site path Zola gives a content file: `foo.md` → `/foo/`, `_index.md`
 * and a page bundle's `index.md` → their directory, `*.<lang>.md` under
 * `/<lang>/`. `slug` and `path` front matter are not supported here; the
 * built-HTML check below refuses a page they would move.
 */
function sitePath(rel) {
  const parts = rel.split(sep);
  let name = parts.pop().replace(/\.md$/, "");
  let lang = null;
  const l = name.match(/^(.*)\.([a-z]{2})$/);
  if (l) [, name, lang] = l;
  if (name !== "_index" && name !== "index") parts.push(name);
  const p = "/" + parts.map((s) => s + "/").join("");
  return { path: lang ? `/${lang}${p}` : p, lang: lang || "en" };
}

// `base_url` as the build used it — read from the home page's canonical link,
// because CI builds with `--base-url` from the Pages deployment, not config.toml.
const home = readFileSync(join(PUBLIC, "index.html"), "utf8");
const canonical = home.match(/<link rel="canonical" href="([^"]+)"/);
if (!canonical) throw new Error("public/index.html has no canonical link — run `zola build` first");
const BASE = canonical[1].replace(/\/$/, "");

/** Resolve `@/…` links and bundle-relative assets outside code fences. */
function resolveLinks(body, pageUrl) {
  let fenced = false;
  return body
    .split("\n")
    .map((line) => {
      if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
      if (fenced) return line;
      return line
        .replace(/(\]\(|\]:\s*)@\/([^)\s#]+)(#[^)\s]*)?/g, (_, lead, target, anchor = "") => {
          return lead + BASE + sitePath(target.split("/").join(sep)).path + anchor;
        })
        .replace(/(\]\()(?![a-z][a-z0-9+.-]*:|[#/@])([^)\s]+)/gi, (_, lead, target) => lead + new URL(target, pageUrl).href);
    })
    .join("\n");
}

const pages = [];
for (const file of walk(CONTENT)) {
  const rel = relative(CONTENT, file);
  const { fm, body } = frontMatter(readFileSync(file, "utf8"), rel);
  const { path, lang } = sitePath(rel);
  if (path === "/" || path === `/${lang}/`) continue; // the landing page is a template, not prose
  if (!existsSync(join(PUBLIC, path, "index.html"))) {
    throw new Error(`${rel}: expected the built page at public${path}index.html — does Zola place it elsewhere?`);
  }
  const url = BASE + path;
  // Every content file opens with a `# Title` repeating its front matter;
  // the twin's own heading replaces it.
  const text = resolveLinks(body.replace(/^\s*# [^\n]*\n/, ""), url).trim();
  const twin =
    `# ${fm.title}\n\n` +
    (fm.description ? `> ${fm.description}\n\n` : "") +
    `Source: ${url}\n\n` +
    `${text}\n`;
  writeFileSync(join(PUBLIC, path, "index.md"), twin);
  const group = rel.split(sep)[1]?.replace(/\.md$/, "") ?? "";
  pages.push({ rel, url, twinUrl: url + "index.md", twin, lang, group, ...fm });
}

// The docs root, then each section's index, then its pages by weight.
const rank = (p) => (p.rel === join("docs", "_index.md") ? 0 : /(^|\/)_index(\.\w+)?\.md$/.test(p.rel) ? 1 : 2);
pages.sort(
  (a, b) =>
    rank(a) - rank(b) ||
    (a.weight ?? 1e9) - (b.weight ?? 1e9) ||
    a.title.localeCompare(b.title),
);

// ── llms.txt ─────────────────────────────────────────────────────────────────
const config = readFileSync(join(SITE, "config.toml"), "utf8");
const siteDesc = JSON.parse(config.match(/^description\s*=\s*(".*")\s*$/m)[1]);
const entry = (p) => `- [${p.title}](${p.twinUrl})${p.description ? `: ${p.description}` : ""}`;
const listed = new Set();
let llms =
  `# mako\n\n> ${siteDesc}\n\n` +
  `Every page below is the Markdown source of a page on ${BASE}/ — append \`index.md\` to any page URL for the same. ` +
  `\`llms-full.txt\` holds all of them in one file.\n`;
for (const [key, heading] of GROUPS) {
  const inGroup = pages.filter(
    (p) => p.lang === "en" && (p.group === key || (key === "guide" && p.rel === join("docs", "_index.md"))),
  );
  if (!inGroup.length) continue;
  llms += `\n## ${heading}\n\n` + inGroup.map(entry).join("\n") + "\n";
  inGroup.forEach((p) => listed.add(p));
}
const translated = pages.filter((p) => p.lang !== "en");
if (translated.length) {
  llms += `\n## Deutsch\n\n` + translated.map(entry).join("\n") + "\n";
  translated.forEach((p) => listed.add(p));
}
const unlisted = pages.filter((p) => !listed.has(p));
if (unlisted.length) {
  throw new Error(`llms.txt has no group for: ${unlisted.map((p) => p.rel).join(", ")} — add it to GROUPS`);
}
writeFileSync(join(PUBLIC, "llms.txt"), llms);

const ordered = [...listed];
writeFileSync(join(PUBLIC, "llms-full.txt"), ordered.map((p) => p.twin).join("\n\n---\n\n"));

// ── Pagefind ─────────────────────────────────────────────────────────────────
const { index, errors } = await pagefind.createIndex({});
if (errors.length) throw new Error(`pagefind: ${errors.join("; ")}`);
const added = await index.addDirectory({ path: PUBLIC });
if (added.errors.length) throw new Error(`pagefind: ${added.errors.join("; ")}`);
const written = await index.writeFiles({ outputPath: join(PUBLIC, "pagefind") });
if (written.errors.length) throw new Error(`pagefind: ${written.errors.join("; ")}`);
await pagefind.close();

// Only pages marked `data-pagefind-body` are indexed, so the count is exact.
// `addDirectory` counts every HTML file it walked; the entry file counts what
// went into each language's index.
const entryFile = JSON.parse(readFileSync(join(PUBLIC, "pagefind", "pagefind-entry.json"), "utf8"));
const indexed = Object.values(entryFile.languages).reduce((n, l) => n + l.page_count, 0);
if (indexed !== pages.length) {
  throw new Error(
    `pagefind indexed ${indexed} pages, the content tree holds ${pages.length} — ` +
      "a template lost `data-pagefind-body`, or a page is indexed twice",
  );
}

console.log(
  `postbuild: ${pages.length} Markdown twins, llms.txt, llms-full.txt, Pagefind index of ${indexed} pages (${Object.keys(entryFile.languages).join(", ")})`,
);
