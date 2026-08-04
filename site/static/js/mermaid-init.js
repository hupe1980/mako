// Render ```mermaid fenced blocks.
//
// Syntax highlighting is on, so Zola emits `<pre><code data-lang="mermaid">`
// with the diagram source wrapped in colour spans — not the `language-mermaid`
// class mermaid documentation assumes. Reading `textContent` discards the spans
// and gives the original source back; mermaid then needs it inside a
// `<pre class="mermaid">`, so convert first and run afterwards.
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";

document
  .querySelectorAll('pre > code[data-lang="mermaid"], pre > code.language-mermaid')
  .forEach((code) => {
    const pre = code.parentElement;
    const holder = document.createElement("pre");
    holder.className = "mermaid";
    holder.textContent = code.textContent;
    pre.replaceWith(holder);
  });

const dark = document.documentElement.getAttribute("data-theme") === "dark"
  || (matchMedia("(prefers-color-scheme: dark)").matches
      && document.documentElement.getAttribute("data-theme") !== "light");

mermaid.initialize({
  startOnLoad: true,
  theme: dark ? "dark" : "neutral",
  securityLevel: "strict",
  fontFamily: "inherit",
});
