// Lightweight client-side docs search over Zola's elasticlunr index.
// Loads the index on first focus; renders the top matches under the search box.
(function () {
  const box = document.getElementById("search");
  const panel = document.getElementById("search-results");
  if (!box || !panel) return;

  let index = null;
  let store = {};
  let loading = false;

  function baseUrl() {
    // Derive the site base path from the page (works under /mako/ on Pages).
    const m = document.querySelector('link[rel="canonical"]');
    return "";
  }

  async function load() {
    if (index || loading) return;
    loading = true;
    try {
      const res = await fetch(new URL("search_index.en.json", document.baseURI));
      const raw = await res.json();
      // Zola ships elasticlunr globally when its JS is present; we do a manual
      // scan instead to avoid the extra dependency.
      store = {};
      (raw.documents || []).forEach((d) => { store[d.id] = d; });
      index = raw;
    } catch (e) {
      index = { documents: [] };
    }
    loading = false;
  }

  function search(q) {
    q = q.trim().toLowerCase();
    if (!q) return [];
    const terms = q.split(/\s+/);
    const docs = (index && index.documents) || [];
    const scored = [];
    for (const d of docs) {
      const hay = ((d.title || "") + " " + (d.body || "")).toLowerCase();
      let score = 0;
      for (const t of terms) {
        const i = hay.indexOf(t);
        if (i === -1) { score = -1; break; }
        score += (d.title || "").toLowerCase().includes(t) ? 5 : 1;
      }
      if (score > 0) scored.push({ d, score });
    }
    scored.sort((a, b) => b.score - a.score);
    return scored.slice(0, 8).map((s) => s.d);
  }

  function snippet(body, q) {
    if (!body) return "";
    const i = body.toLowerCase().indexOf(q.trim().split(/\s+/)[0]);
    const start = Math.max(0, i - 40);
    return (start > 0 ? "…" : "") + body.slice(start, start + 120).trim() + "…";
  }

  function render(results, q) {
    if (!q.trim()) { panel.hidden = true; return; }
    if (!results.length) {
      panel.innerHTML = '<div class="sr-empty">No matches.</div>';
      panel.hidden = false; return;
    }
    panel.innerHTML = results.map((d) =>
      `<a href="${d.id}"><span class="sr-title">${d.title || d.id}</span>` +
      `<span class="sr-snip">${snippet(d.body, q)}</span></a>`
    ).join("");
    panel.hidden = false;
  }

  box.addEventListener("focus", load, { once: true });
  box.addEventListener("input", async () => { await load(); render(search(box.value), box.value); });
  document.addEventListener("click", (e) => {
    if (!panel.contains(e.target) && e.target !== box) panel.hidden = true;
  });
  // "/" focuses search.
  document.addEventListener("keydown", (e) => {
    if (e.key === "/" && document.activeElement !== box) { e.preventDefault(); box.focus(); }
    if (e.key === "Escape") panel.hidden = true;
  });
})();
