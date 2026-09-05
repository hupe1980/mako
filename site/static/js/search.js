// Client-side docs search over Zola's `fuse_json` search index.
//
// The index is fetched once, on first focus of the search box, and scanned
// here: 46 documents is far below the size where a real inverted index earns
// its download. `config.toml` therefore builds `fuse_json` — a flat array of
// {url, title, description, body, path} — rather than `elasticlunr_json`,
// whose inverted index this file would never read.
//
// It used to read `raw.documents`, a key `elasticlunr_json` does not have
// (documents live under `documentStore.docs`), so every query rendered
// "No matches." and the search box had never worked.
(function () {
  const box = document.getElementById("search");
  const panel = document.getElementById("search-results");
  if (!box || !panel) return;

  const MAX_RESULTS = 8;

  let docs = null;
  let pending = null;
  let selected = -1;

  function load() {
    if (docs) return Promise.resolve(docs);
    if (pending) return pending;
    // `document.baseURI` resolves under the /mako/ path prefix on GitHub Pages
    // and under / on a custom domain, so no base path is hard-coded.
    pending = fetch(new URL("search_index.en.json", document.baseURI))
      .then((res) => (res.ok ? res.json() : []))
      .then((raw) => {
        // Tolerate either shape: `fuse_json` is the array, `elasticlunr_json`
        // hides the same documents one level down. Neither is guessed at.
        docs = Array.isArray(raw)
          ? raw
          : Object.values((raw && raw.documentStore && raw.documentStore.docs) || {});
        docs = docs.map((d) => ({
          url: d.url || d.id,
          title: d.title || "",
          description: d.description || "",
          body: d.body || "",
          hay: ((d.title || "") + "\n" + (d.description || "") + "\n" + (d.body || "")).toLowerCase(),
          titleLc: (d.title || "").toLowerCase(),
          descLc: (d.description || "").toLowerCase(),
        }));
        return docs;
      })
      .catch(() => (docs = []));
    return pending;
  }

  function search(q) {
    const terms = q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length || !docs) return [];
    const scored = [];
    for (const d of docs) {
      let score = 0;
      let matchedAll = true;
      for (const t of terms) {
        if (d.hay.indexOf(t) === -1) {
          matchedAll = false;
          break;
        }
        // A term in the title is what the page is about; in the description,
        // what it claims to be; in the body, how much of the page is about it.
        // The body bonus is capped so one long page cannot outrank the page
        // that actually names the thing.
        if (d.titleLc.indexOf(t) !== -1) score += 12;
        if (d.descLc.indexOf(t) !== -1) score += 6;
        score += Math.min(6, d.hay.split(t).length - 1);
      }
      if (matchedAll) scored.push({ d, score });
    }
    scored.sort((a, b) => b.score - a.score || a.d.title.localeCompare(b.d.title));
    return scored.slice(0, MAX_RESULTS).map((s) => s.d);
  }

  const esc = (s) =>
    String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  // A window of body text around the first hit, with the term marked. Result
  // text comes from the index, so it is escaped before the <mark> goes in.
  function snippet(doc, term) {
    const source = doc.body || doc.description;
    if (!source) return "";
    const i = source.toLowerCase().indexOf(term);
    if (i === -1) return esc(source.slice(0, 120).trim()) + "…";
    const start = Math.max(0, i - 45);
    const raw = source.slice(start, start + 150).trim();
    const at = raw.toLowerCase().indexOf(term);
    const out =
      at === -1
        ? esc(raw)
        : esc(raw.slice(0, at)) + "<mark>" + esc(raw.slice(at, at + term.length)) + "</mark>" + esc(raw.slice(at + term.length));
    return (start > 0 ? "…" : "") + out + "…";
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
    const term = q.trim().toLowerCase().split(/\s+/)[0];
    panel.innerHTML = results
      .map(
        (d, i) =>
          `<a id="sr-${i}" role="option" aria-selected="false" href="${esc(d.url)}">` +
          `<span class="sr-title">${esc(d.title || d.url)}</span>` +
          `<span class="sr-snip">${snippet(d, term)}</span></a>`
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

  const run = () => load().then(() => render(search(box.value), box.value));

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
