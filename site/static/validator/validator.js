// The validator page: a market message checked in this tab by mako's validator,
// compiled to WebAssembly (tools/mako-wasm). Nothing leaves the tab — the page
// declares `connect-src 'self'` and every value reaches the DOM as text.

import { MAX, el, load, readFile, showCurrentTab } from "./module.js";

showCurrentTab();

const root = document.querySelector(".ps");
const UI = JSON.parse(document.getElementById("ps-ui").textContent);
const LANG = root.dataset.lang === "de" ? "de-DE" : "en-GB";

const $ = (id) => document.getElementById(id);
const text = $("ps-text");
const status = $("ps-status");
const result = $("ps-result");

const fill = (s, vars) => s.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");

// ── The module ───────────────────────────────────────────────────────────────

let call = null;

// ── Input ────────────────────────────────────────────────────────────────────

let sample = null; // the bundled sample on screen, if unchanged

/** Grow the editor with its message, up to the CSS max-height. */
function fit() {
  text.style.height = "auto";
  text.style.height = `${text.scrollHeight + 2}px`;
}

function setText(value, from = null) {
  text.value = value;
  sample = from;
  fit();
  run();
}

$("ps-file").addEventListener("change", async (e) => {
  const file = e.target.files[0];
  if (!file) return;
  try {
    setText(await readFile(file, UI.too_big));
  } catch (err) {
    status.textContent = err.message;
  }
  e.target.value = "";
});

const drop = $("ps-drop");
drop.addEventListener("dragover", (e) => {
  e.preventDefault();
  drop.classList.add("is-over");
});
drop.addEventListener("dragleave", () => drop.classList.remove("is-over"));
drop.addEventListener("drop", async (e) => {
  e.preventDefault();
  drop.classList.remove("is-over");
  const file = e.dataTransfer.files[0];
  if (!file) return;
  try {
    setText(await readFile(file, UI.too_big));
  } catch (err) {
    status.textContent = err.message;
  }
});

let timer;
text.addEventListener("input", () => {
  sample = null;
  fit();
  clearTimeout(timer);
  timer = setTimeout(run, 180);
});

// ── Checking ─────────────────────────────────────────────────────────────────

let empfangen = null; // the arrival time the user set; null = now
let current = 0; // which message of the interchange is on screen

function run() {
  if (!call) return;
  const value = text.value;
  if (!value.trim()) {
    result.hidden = true;
    root.classList.remove("has-result");
    return;
  }
  if (value.length > MAX) {
    show({ refused: { code: "eingabe_zu_gross", message: UI.too_big } }, value);
    return;
  }
  const at = (empfangen ?? new Date()).toISOString();
  show(call({ op: "check", text: value, empfangen: at }), value);
}

// ── Rendering ────────────────────────────────────────────────────────────────

const berlin = new Intl.DateTimeFormat(LANG, {
  timeZone: "Europe/Berlin",
  weekday: "short",
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});
const day = new Intl.DateTimeFormat(LANG, { day: "2-digit", month: "2-digit", year: "numeric" });
const relative = new Intl.RelativeTimeFormat(LANG, { numeric: "auto" });

function inTime(iso) {
  const ms = new Date(iso) - (empfangen ?? new Date());
  const min = Math.round(ms / 60000);
  if (Math.abs(min) < 90) return relative.format(min, "minute");
  const h = Math.round(min / 60);
  if (Math.abs(h) < 36) return relative.format(h, "hour");
  return relative.format(Math.round(h / 24), "day");
}

/** BDEW (`99…`), DVGW (`98…`) or GS1 for another 13-digit code. */
function codeArt(id) {
  if (!id) return null;
  if (/^99\d{11}$/.test(id)) return "BDEW";
  if (/^98\d{11}$/.test(id)) return "DVGW";
  if (/^\d{13}$/.test(id)) return "GLN";
  return null;
}

function show(answer, source) {
  result.hidden = false;
  root.classList.add("has-result");
  result.replaceChildren();
  if (answer.refused) {
    const r = answer.refused;
    result.append(
      el("div", { class: "ps-verdict is-refused" },
        el("div", { class: "ps-verdict-icon", "aria-hidden": "true" }, "?"),
        el("div", {},
          el("h2", {}, UI.refusals[r.code] ?? UI.refused),
          el("p", { class: "ps-note" }, r.message),
        ),
      ),
    );
    return;
  }
  const messages = answer.ok.nachrichten;
  if (current >= messages.length) current = 0;
  if (messages.length > 1) {
    result.append(
      el("div", { class: "ps-tabs", role: "tablist", "aria-label": UI.messages },
        messages.map((m, i) =>
          el("button", {
            role: "tab",
            "aria-selected": String(i === current),
            class: `ps-tab ${m.gueltig ? "ok" : "bad"}`,
            onclick: () => { current = i; run(); },
          }, `${i + 1} · ${m.typ ?? "?"} ${m.pid ?? ""}`),
        ),
      ),
    );
  }
  const m = messages[current];
  if (m.fehler) {
    result.append(el("p", { class: "ps-alert" }, m.fehler));
    return;
  }
  result.append(verdict(m), body(m, source));
}

function verdict(m) {
  const counts = { fehler: 0, warnung: 0, hinweis: 0 };
  for (const b of m.befunde) counts[b.schwere]++;
  const ok = m.gueltig;
  const headline = ok
    ? UI.valid
    : [
        counts.fehler && fill(counts.fehler === 1 ? UI.one_error : UI.errors, { n: counts.fehler }),
        counts.warnung && fill(counts.warnung === 1 ? UI.one_warning : UI.warnings, { n: counts.warnung }),
      ].filter(Boolean).join(" · ") || UI.invalid;

  const superseded = m.gilt_bis && new Date(m.gilt_bis) < new Date();
  const fv = m.formatversion &&
    el("span", { class: `ps-chip ${superseded ? "is-old" : ""}`, title: m.ahb ? `AHB ${m.ahb}` : null },
      `${UI.format} ${m.formatversion}`,
      m.gilt_ab && ` · ${fill(superseded ? UI.superseded : UI.valid_from, {
        date: day.format(new Date(superseded ? m.gilt_bis : m.gilt_ab)),
      })}`,
    );
  const party = (id) => id && el("span", { class: "ps-party" },
    codeArt(id) && el("span", { class: "ps-party-art" }, codeArt(id)), id);

  return el("div", { class: `ps-verdict ${ok ? "is-ok" : "is-bad"}` },
    el("div", { class: "ps-verdict-icon", "aria-hidden": "true" }, ok ? "✓" : "✗"),
    el("div", { class: "ps-verdict-main" },
      el("h2", {}, headline),
      el("p", { class: "ps-identity" },
        m.typ && el("span", { class: "ps-chip is-strong" }, m.typ),
        m.pid && el("span", { class: "ps-chip is-strong" }, `PID ${m.pid}`),
        m.anwendungsfall && el("span", { class: "ps-af" }, m.anwendungsfall),
        m.kommunikation && el("span", { class: "ps-note" }, m.kommunikation),
      ),
      el("p", { class: "ps-meta" },
        fv,
        (m.absender || m.empfaenger) && el("span", { class: "ps-route" },
          party(m.absender), el("span", { "aria-label": UI.to }, " → "), party(m.empfaenger)),
      ),
    ),
    el("div", { class: "ps-verdict-actions" },
      el("button", { class: "btn btn-ghost", type: "button", onclick: share }, UI.share),
      el("output", { id: "ps-shared", class: "ps-note" }),
    ),
  );
}

function body(m, source) {
  const findingsBySeg = new Map();
  m.befunde.forEach((b, i) => {
    if (b.segment != null) {
      if (!findingsBySeg.has(b.segment)) findingsBySeg.set(b.segment, []);
      findingsBySeg.get(b.segment).push(i);
    }
  });

  const inspector = el("div", { class: "ps-inspector", "aria-live": "polite" }, UI.inspector_hint);
  const inspect = (seg, v) => {
    // Through `el`, which drops the `false` an absent part evaluates to.
    inspector.replaceChildren(...el("div", {},
      el("span", { class: "ps-chip is-mono" }, seg.tag + (v ? ` · DE ${v.de}` : "")),
      el("strong", {}, v ? v.name : seg.name ?? UI.unnamed),
      v && v.bedeutung && el("span", { class: "ps-meaning" }, v.bedeutung),
      !v && seg.status && el("span", { class: "ps-note" }, `AHB: ${seg.status.join(" ")}`),
      v && seg.name && el("span", { class: "ps-note" }, `${UI.in} ${seg.name}`),
    ).childNodes);
  };

  const rows = m.segmente.map((seg, i) => {
    const raw = source.slice(seg.start, seg.end).replace(/\s+$/, "");
    const code = el("code", { class: "ps-raw" });
    let at = seg.start;
    for (const v of seg.werte) {
      if (v.start < at || v.end > seg.start + raw.length) continue;
      code.append(source.slice(at, v.start));
      code.append(el("span", {
        class: `ps-v ${v.bedeutung ? "is-coded" : ""}`,
        tabindex: "0",
        onmouseenter: () => inspect(seg, v),
        onfocus: () => inspect(seg, v),
      }, source.slice(v.start, v.end)));
      at = v.end;
    }
    code.append(source.slice(at, seg.start + raw.length));
    const hits = findingsBySeg.get(i) ?? [];
    const worst = hits.some((h) => m.befunde[h].schwere === "fehler") ? "fehler"
      : hits.length ? "warnung" : null;
    return el("li", {
      id: `ps-seg-${i}`,
      class: `ps-seg ${worst ? `has-${worst}` : ""}`,
      onmouseenter: () => inspect(seg, null),
    },
      el("span", { class: "ps-ln", "aria-hidden": "true" }, i + 1),
      code,
      el("span", { class: "ps-segname", title: seg.name }, seg.name ?? ""),
      hits.length > 0 && el("button", {
        class: `ps-flag ${worst}`,
        type: "button",
        "aria-label": fill(UI.goto_finding, { n: hits.length }),
        onclick: () => focusFinding(hits[0]),
      }, hits.length),
    );
  });

  const message = el("section", { class: "ps-message", "aria-label": UI.message },
    el("ol", { class: "ps-segs" }, rows),
    inspector,
  );
  return el("div", { class: "ps-grid" }, message, aside(m));
}

function focusSegment(i) {
  const row = $(`ps-seg-${i}`);
  if (!row) return;
  row.scrollIntoView({ block: "center", behavior: "smooth" });
  row.classList.remove("is-flash");
  void row.offsetWidth;
  row.classList.add("is-flash");
  row.querySelector(".ps-v")?.focus({ preventScroll: true });
}

function focusFinding(i) {
  const f = $(`ps-finding-${i}`);
  f?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  f?.focus({ preventScroll: true });
}

function aside(m) {
  const findings = m.befunde.length
    ? el("ol", { class: "ps-findings" },
        m.befunde.map((b, i) =>
          el("li", { id: `ps-finding-${i}`, class: `ps-finding is-${b.schwere}`, tabindex: "-1" },
            el("p", { class: "ps-finding-head" },
              el("span", { class: "ps-dot", "aria-hidden": "true" }),
              el("span", { class: "ps-chip" }, b.herkunft),
              el("span", { class: "sr-only" }, UI.severity[b.schwere]),
              b.segment != null
                ? el("button", { class: "ps-link", type: "button", onclick: () => focusSegment(b.segment) },
                    fill(UI.line, { n: b.segment + 1 }))
                : el("span", { class: "ps-note" }, UI.whole_message),
            ),
            el("p", { class: "ps-finding-text" }, b.text),
            b.bedingungen.map((c) => el("p", { class: "ps-condition" }, el("strong", {}, `[${c.nr}]`), " ", c.text)),
            b.regel && el("p", { class: "ps-rule" }, b.regel),
          ),
        ),
      )
    : el("p", { class: "ps-clean" }, UI.no_findings);

  const blocks = [el("section", { class: "ps-card" }, el("h3", {}, UI.findings), findings)];

  if (m.fristen && m.fristen.length) {
    const input = el("input", {
      type: "datetime-local",
      id: "ps-empfangen",
      value: localValue(empfangen ?? new Date()),
      onchange: (e) => {
        empfangen = e.target.value ? new Date(e.target.value) : null;
        run();
      },
    });
    blocks.push(el("section", { class: "ps-card" },
      el("h3", {}, UI.deadlines),
      el("label", { class: "ps-received" }, UI.received, input,
        empfangen && el("button", { class: "ps-link", type: "button", onclick: () => { empfangen = null; run(); } }, UI.now)),
      el("ul", { class: "ps-fristen" },
        m.antwort && m.antwort.faellig && el("li", {},
          el("span", { class: "ps-chip is-strong" }, m.antwort.durch),
          el("span", { class: "ps-due" }, berlin.format(new Date(m.antwort.faellig)),
            el("span", { class: "ps-note" }, ` · ${inTime(m.antwort.faellig)}`)),
          el("span", { class: "ps-source" }, m.antwort.quelle.split(" — ")[0],
            m.antwort.entscheidungsbaum && ` · ${m.antwort.entscheidungsbaum}`),
        ),
        m.fristen.map((f) => el("li", {},
          el("span", { class: "ps-chip is-strong" }, f.was),
          el("span", { class: "ps-due" }, berlin.format(new Date(f.faellig)),
            el("span", { class: "ps-note" }, ` · ${inTime(f.faellig)}`)),
          el("span", { class: "ps-source" }, f.quelle, f.fall && ` · ${f.fall}`),
        )),
      ),
      el("p", { class: "ps-note" }, UI.deadline_note),
    ));
  }

  if (m.antworten || m.route || m.aperak) {
    blocks.push(el("section", { class: "ps-card" },
      el("h3", {}, UI.answers),
      m.route && el("p", { class: "ps-route-line" }, `${UI.routed_to} `,
        el("code", {}, `${m.route.crate} · ${m.route.workflow}`)),
      m.aperak && el("p", { class: "ps-route-line" }, el("strong", {}, UI.aperak[m.aperak.art]),
        el("span", { class: "ps-note" }, ` · ${m.aperak.quelle}`)),
      m.antworten && el("ul", { class: "ps-answers" },
        m.antworten.map((a) => el("li", {}, el("span", { class: "ps-chip is-strong" }, a.pid), a.name ?? "")),
      ),
    ));
  }
  return el("aside", { class: "ps-aside" }, blocks);
}

/** `Date` → the `datetime-local` value in this browser's zone. */
function localValue(d) {
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

// ── Sharing ──────────────────────────────────────────────────────────────────

function b64url(bytes) {
  let s = "";
  // In chunks: spreading a large message into one call overflows the stack.
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
const unb64url = (s) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

async function deflate(s) {
  const stream = new Blob([new TextEncoder().encode(s)]).stream().pipeThrough(new CompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function inflate(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  const reader = stream.getReader();
  const parts = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > MAX) throw new Error(UI.too_big);
    parts.push(value);
  }
  return new TextDecoder().decode(await new Blob(parts).arrayBuffer());
}

async function share() {
  let hash;
  if (sample) {
    hash = `#beispiel=${encodeURIComponent(sample)}`;
  } else {
    const dialog = $("ps-share-dialog");
    dialog.showModal();
    const choice = await new Promise((resolve) =>
      dialog.addEventListener("close", () => resolve(dialog.returnValue), { once: true }));
    if (choice !== "ok") return;
    hash = `#nachricht=${b64url(await deflate(text.value))}`;
  }
  history.replaceState(null, "", hash);
  const out = $("ps-shared");
  try {
    await navigator.clipboard.writeText(location.href);
    out.textContent = UI.copied;
  } catch {
    out.textContent = UI.link_in_address;
  }
}

async function fromHash() {
  const params = new URLSearchParams(location.hash.slice(1));
  if (params.has("beispiel")) {
    const name = params.get("beispiel");
    const a = call({ op: "sample", name });
    if (a.ok) setText(a.ok.text, name);
  } else if (params.has("nachricht")) {
    try {
      setText(await inflate(unb64url(params.get("nachricht"))));
      status.textContent = UI.from_link;
    } catch {
      status.textContent = UI.bad_link;
    }
  }
}

// ── Start ────────────────────────────────────────────────────────────────────

load(root.dataset.wasm)
  .then(async (loaded) => {
    call = loaded;
    if (!call) {
      $("ps-unsupported").hidden = false;
      return;
    }
    status.textContent = UI.ready;
    const list = $("ps-sample-list");
    for (const s of call({ op: "samples" }).ok) {
      list.append(el("button", {
        class: "ps-chip-btn",
        type: "button",
        onclick: () => {
          setText(call({ op: "sample", name: s.name }).ok.text, s.name);
          history.replaceState(null, "", `#beispiel=${encodeURIComponent(s.name)}`);
        },
      }, s.title));
    }
    await fromHash();
    if (text.value) run();
  })
  .catch((err) => {
    status.textContent = `${UI.load_failed} (${err.message})`;
  });
