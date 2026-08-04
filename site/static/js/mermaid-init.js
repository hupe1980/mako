// Render ```mermaid fenced blocks.
//
// Syntax highlighting is on, so Zola emits `<pre><code data-lang="mermaid">`
// with the diagram source wrapped in colour spans — not the `language-mermaid`
// class mermaid documentation assumes. Reading `textContent` discards the spans
// and gives the original source back; mermaid then needs it inside a
// `<pre class="mermaid">`, so convert first and run afterwards.
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";

// Keep the original source: mermaid replaces a holder's innerHTML with the
// rendered SVG, so re-rendering (on a theme switch) needs the text back.
const SOURCE = new WeakMap();

function collectHolders() {
  document
    .querySelectorAll('pre > code[data-lang="mermaid"], pre > code.language-mermaid')
    .forEach((code) => {
      const pre = code.parentElement;
      const holder = document.createElement("pre");
      holder.className = "mermaid";
      holder.textContent = code.textContent;
      pre.replaceWith(holder);
    });

  const holders = [...document.querySelectorAll("pre.mermaid")];
  holders.forEach((h) => {
    if (!SOURCE.has(h)) SOURCE.set(h, h.textContent);
  });
  return holders;
}

function darkMode() {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "dark") return true;
  if (attr === "light") return false;
  return matchMedia("(prefers-color-scheme: dark)").matches;
}

async function render(holders) {
  // `startOnLoad` is deliberately off. Mermaid registers its auto-run on
  // `window.load` when the *module* evaluates, and this module is imported
  // dynamically from a CDN — so on a cold cache it evaluates after `load` has
  // already fired, the listener never runs, and the page shows raw diagram
  // source. Driving `run()` ourselves removes that race.
  mermaid.initialize({
    startOnLoad: false,
    theme: darkMode() ? "dark" : "neutral",
    securityLevel: "strict",
    fontFamily: "inherit",
  });
  try {
    await mermaid.run({ nodes: holders });
  } catch (e) {
    // One malformed diagram must not blank the rest of the page.
    console.error("mermaid: render failed", e);
  }
}

const holders = collectHolders();
if (holders.length) {
  await render(holders);

  // Re-render on theme switch: mermaid bakes the palette into the SVG, so
  // diagrams would otherwise keep the theme they booted with. Restore the
  // source and clear `data-processed`, which mermaid uses to skip nodes.
  let theme = darkMode();
  new MutationObserver(async () => {
    if (darkMode() === theme) return;
    theme = darkMode();
    holders.forEach((h) => {
      h.removeAttribute("data-processed");
      h.textContent = SOURCE.get(h);
    });
    await render(holders);
  }).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
}
