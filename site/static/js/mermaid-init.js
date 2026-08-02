// Render ```mermaid fenced blocks. Zola emits them as <pre><code class="language-mermaid">;
// mermaid expects <pre class="mermaid">, so convert first, then run.
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";

document.querySelectorAll("pre > code.language-mermaid").forEach((code) => {
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
