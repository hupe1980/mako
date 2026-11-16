// Client-side docs search over the Pagefind index `tools/postbuild.mjs` writes
// to `pagefind/`.
//
// Pagefind's bundle is imported on first focus of the search box and fetches
// only the index chunks a query touches, so a visitor who never searches
// downloads nothing. It reads the page's `<html lang>` and searches the index
// built for that language, which stems German pages as German.
(function () {
  const box = document.getElementById("search");
  const panel = document.getElementById("search-results");
  if (!box || !panel) return;

  const MAX_RESULTS = 8;

  // The site root, from this script's own URL (`<root>js/search.js`): it is
  // `/mako/` on the GitHub Pages project path and `/` on a domain of its own,
  // so no base path is hard-coded.
  const ROOT = new URL("..", document.currentScript.src);

  let pagefind = null;
  let pending = null;
  let selected = -1;
  let latest = 0;

  function load() {
    if (pagefind) return Promise.resolve(pagefind);
    if (pending) return pending;
    pending = import(new URL("pagefind/pagefind.js", ROOT).href)
      .then(async (pf) => {
        // Result URLs are relative to the indexed `public/`; the base URL puts
        // the project path back in front of them.
        await pf.options({ baseUrl: ROOT.pathname });
        pf.init();
        return (pagefind = pf);
      })
      .catch(() => {
        pending = null;
        return null;
      });
    return pending;
  }

  async function search(q) {
    const pf = await load();
    if (!pf || !q.trim()) return [];
    // Debounced, so a word typed letter by letter does not fetch the excerpts
    // of every prefix; a superseded query answers `null` and renders nothing.
    const found = await pf.debouncedSearch(q, {}, 150);
    if (!found) return null;
    return Promise.all(found.results.slice(0, MAX_RESULTS).map((r) => r.data()));
  }

  const esc = (s) =>
    String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  // Pagefind's excerpt is HTML with the hits in <mark>. Decode it to text,
  // escape that, and put back only the marks — nothing else from the index
  // reaches the page as markup.
  const decoder = document.createElement("textarea");
  function snippet(excerpt) {
    decoder.innerHTML = String(excerpt || "")
      .replace(/<mark>/g, "\u0001")
      .replace(/<\/mark>/g, "\u0002")
      .replace(/<[^>]*>/g, "");
    return esc(decoder.value).replace(/\u0001/g, "<mark>").replace(/\u0002/g, "</mark>");
  }

  function setExpanded(open) {
    panel.hidden = !open;
    box.setAttribute("aria-expanded", open ? "true" : "false");
    if (!open) {
      selected = -1;
      box.removeAttribute("aria-activedescendant");
    }
  }

  function render(results, q) {
    selected = -1;
    if (!q.trim()) return setExpanded(false);
    if (!results.length) {
      panel.innerHTML = '<div class="sr-empty">No matches for “' + esc(q.trim()) + "”.</div>";
      return setExpanded(true);
    }
    panel.innerHTML = results
      .map(
        (d, i) =>
          `<a id="sr-${i}" role="option" aria-selected="false" href="${esc(d.url)}">` +
          `<span class="sr-title">${esc((d.meta && d.meta.title) || d.url)}</span>` +
          `<span class="sr-snip">${snippet(d.excerpt)}</span></a>`
      )
      .join("");
    setExpanded(true);
  }

  function move(delta) {
    const items = panel.querySelectorAll("a");
    if (!items.length) return;
    if (selected >= 0) {
      items[selected].classList.remove("sel");
      items[selected].setAttribute("aria-selected", "false");
    }
    selected = (selected + delta + items.length) % items.length;
    const el = items[selected];
    el.classList.add("sel");
    el.setAttribute("aria-selected", "true");
    box.setAttribute("aria-activedescendant", el.id);
    if (el.scrollIntoView) el.scrollIntoView({ block: "nearest" });
  }

  // Answers can arrive out of order while the reader types; only the newest
  // query may render.
  const run = () => {
    const q = box.value;
    const ticket = ++latest;
    search(q).then((results) => {
      if (results && ticket === latest) render(results, q);
    });
  };

  box.addEventListener("focus", load, { once: true });
  box.addEventListener("input", run);

  box.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      panel.hidden ? run() : move(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Enter" && selected >= 0) {
      e.preventDefault();
      panel.querySelectorAll("a")[selected].click();
    } else if (e.key === "Escape") {
      setExpanded(false);
      box.blur();
    }
  });

  document.addEventListener("click", (e) => {
    if (!panel.contains(e.target) && e.target !== box) setExpanded(false);
  });

  // "/" focuses search, unless the caret is already in a field.
  document.addEventListener("keydown", (e) => {
    const tag = (document.activeElement && document.activeElement.tagName) || "";
    if (e.key === "/" && tag !== "INPUT" && tag !== "TEXTAREA") {
      e.preventDefault();
      box.focus();
    }
  });
})();
