// The validator's small tools: an Antwortcode in its Entscheidungsbaum, a
// process's Antwortfrist, an identifier's check digit. Each answers as you
// type, keeps its question in the address fragment so a result can be linked,
// and renders every value as text (module.js `el`).

import { el, load, showCurrentTab } from "./module.js";

showCurrentTab();

const root = document.querySelector(".vt");
const UI = JSON.parse(document.getElementById("ps-ui").textContent);
const LANG = root.dataset.lang === "de" ? "de-DE" : "en-GB";
const $ = (id) => document.getElementById(id);
const status = $("vt-status");
const result = $("vt-result");
const fill = (s, vars) => s.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");
const due = new Intl.DateTimeFormat(LANG, {
  timeZone: "Europe/Berlin", weekday: "short", day: "2-digit", month: "2-digit", year: "numeric",
  hour: "2-digit", minute: "2-digit",
});
const relative = new Intl.RelativeTimeFormat(LANG, { numeric: "auto" });

let call = null;

/** Write the question into the fragment, so the address shares the answer. */
function remember(params) {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v) p.set(k, v);
  history.replaceState(null, "", p.toString() ? `#${p}` : location.pathname);
}
const asked = () => new URLSearchParams(location.hash.slice(1));

function refusal(r) {
  return el("p", { class: "ps-alert" }, r.message);
}

// ── Antwortcode ──────────────────────────────────────────────────────────────

function codes() {
  const ebd = $("vt-ebd");
  const code = $("vt-code");
  const list = $("vt-ebd-list");
  const baeume = call({ op: "codes" }).ok.baeume;
  for (const b of baeume) list.append(el("option", { value: b.ebd }, fill(UI.tree_codes, { n: b.codes })));

  const card = (t, big) =>
    el("article", { class: `vt-card ${t.zustimmung === true ? "is-yes" : t.zustimmung === false ? "is-no" : ""} ${big ? "is-big" : ""}` },
      el("p", { class: "vt-card-head" },
        el("span", { class: "ps-chip is-mono" }, t.ebd),
        el("strong", { class: "vt-code" }, t.code),
        el("span", { class: "vt-art" }, t.art),
      ),
      el("p", { class: "vt-meaning" }, t.bedeutung),
      t.bemerkung_noetig && el("p", { class: "ps-note" }, UI.needs_text),
    );

  const run = () => {
    const e = ebd.value.trim();
    const c = code.value.trim();
    remember({ ebd: e, code: c });
    result.replaceChildren();
    if (!e && !c) return;
    const a = call({ op: "codes", ebd: e || null, code: c || null });
    if (a.refused) return result.append(refusal(a.refused));
    const t = a.ok.treffer;
    if (!t.length) return result.append(el("p", { class: "ps-alert" }, e && c ? fill(UI.not_in_tree, { code: c.toUpperCase(), ebd: e.toUpperCase() }) : UI.no_match));
    if (e && c) return result.append(card(t[0], true));
    if (c && !e) result.append(el("p", { class: "vt-hint" }, fill(UI.many_trees, { code: c.toUpperCase(), n: t.length })));
    result.append(el("div", { class: "vt-grid" }, t.map((x) => card(x, false))));
  };
  ebd.addEventListener("input", run);
  code.addEventListener("input", run);
  const q = asked();
  ebd.value = q.get("ebd") ?? "";
  code.value = q.get("code") ?? "";
  run();
  (ebd.value ? code : ebd).focus();
}

// ── Frist ────────────────────────────────────────────────────────────────────

function fristen() {
  const suche = $("vt-suche");
  const eingang = $("vt-empfangen");
  const p = (n) => String(n).padStart(2, "0");
  const now = new Date();
  eingang.value = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}T${p(now.getHours())}:${p(now.getMinutes())}`;

  const run = () => {
    const s = suche.value.trim();
    remember({ pid: s });
    result.replaceChildren();
    const at = eingang.value ? new Date(eingang.value) : new Date();
    const a = call({ op: "frist", suche: s, empfangen: at.toISOString() });
    if (a.refused) return result.append(refusal(a.refused));
    const t = a.ok.treffer;
    if (!t.length) return result.append(el("p", { class: "ps-alert" }, UI.no_match));
    const exact = t.find((x) => x.faellig);
    if (exact) {
      const days = Math.round((new Date(exact.faellig) - at) / 864e5);
      result.append(el("article", { class: "vt-card is-big is-due" },
        el("p", { class: "vt-card-head" }, el("span", { class: "ps-chip is-strong" }, `PID ${exact.pid}`), el("strong", {}, exact.name)),
        el("p", { class: "vt-due" }, fill(UI.answers_by, { who: exact.durch, when: due.format(new Date(exact.faellig)) }),
          el("span", { class: "ps-note" }, ` · ${relative.format(days, "day")}`)),
        el("p", { class: "vt-meaning" }, exact.quelle),
        el("p", { class: "ps-note" }, fill(UI.answers_with, { yes: exact.antworten[0], no: exact.antworten[1] }),
          exact.entscheidungsbaum && ` · ${fill(UI.decided_by, { ebd: exact.entscheidungsbaum })}`),
      ));
      if (t.length === 1) return;
    }
    result.append(el("ul", { class: "vt-list" }, t.filter((x) => x !== exact).map((x) =>
      el("li", {},
        el("button", { class: "vt-row", type: "button", onclick: () => { suche.value = String(x.pid); run(); } },
          el("span", { class: "ps-chip is-strong" }, x.pid),
          el("span", {}, x.name),
          el("span", { class: "ps-note" }, `${x.durch}${x.entscheidungsbaum ? ` · ${x.entscheidungsbaum}` : ""}`),
        )),
    )));
  };
  suche.addEventListener("input", run);
  eingang.addEventListener("change", run);
  suche.value = asked().get("pid") ?? "";
  run();
  suche.focus();
}

// ── ID ───────────────────────────────────────────────────────────────────────

function ids() {
  const input = $("vt-id");
  const run = () => {
    const v = input.value;
    remember({ id: v.trim() });
    result.replaceChildren();
    if (!v.trim()) return;
    const a = call({ op: "id", wert: v });
    if (a.refused) return result.append(refusal(a.refused));
    const r = a.ok;
    const id = v.replace(/\s+/g, "");
    let body;
    if (r.art === "mpid") {
      const ok = r.herkunft === "gln" ? r.gln_pruefziffer : r.bdew_pruefziffer;
      body = [
        el("p", { class: `vt-verdict ${ok ? "is-yes" : "is-no"}` }, ok ? UI.check_ok : UI.check_bad),
        el("p", { class: "ps-note" }, fill(UI.mp_checks, { bdew: r.bdew_pruefziffer ? "✓" : "✗", gln: r.gln_pruefziffer ? "✓" : "✗" })),
        el("p", { class: "ps-note" }, UI.assigned_note),
      ];
    } else {
      const fix = !r.gueltig && r.pruefziffer ? id.slice(0, -1) + r.pruefziffer : null;
      body = [
        el("p", { class: `vt-verdict ${r.gueltig ? "is-yes" : "is-no"}` }, r.gueltig ? UI.check_ok : UI.check_bad),
        fix && el("p", { class: "vt-fix" }, `${UI.would_be} `, el("code", {}, id.slice(0, -1)), el("code", { class: "vt-digit" }, r.pruefziffer)),
      ];
    }
    result.append(el("article", { class: `vt-card is-big ${(r.gueltig ?? (r.herkunft === "gln" ? r.gln_pruefziffer : r.bdew_pruefziffer)) ? "is-yes" : "is-no"}` },
      el("p", { class: "vt-card-head" }, el("strong", {}, UI.kinds[r.herkunft ? `${r.art}_${r.herkunft}` : r.art])),
      body,
    ));
  };
  input.addEventListener("input", run);
  input.value = asked().get("id") ?? "";
  run();
  input.focus();
}

// ── Start ────────────────────────────────────────────────────────────────────

load(root.dataset.wasm)
  .then((c) => {
    call = c;
    if (!call) {
      status.textContent = UI.no_wasm;
      return;
    }
    status.textContent = "";
    ({ codes, fristen, ids })[root.dataset.tool]();
  })
  .catch((err) => {
    status.textContent = `${UI.load_failed} (${err.message})`;
  });
