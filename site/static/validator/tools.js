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

// ── Calculators ─────────────────────────────────────────────────────────────

const num = (x, digits = 2) =>
  new Intl.NumberFormat(LANG, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(Number(x));
const eur = (x) => `${num(x, Number(x) % 1 ? 2 : 0)} €`;
const val = (id) => $(id).value.trim();
const today = () => new Date().toISOString().slice(0, 10);

/** Fill inputs from the fragment, wire `run` to every input, run once. */
function wire(ids, run) {
  const q = asked();
  for (const id of ids) {
    const input = $(id);
    const v = q.get(id.slice(3));
    if (v != null) {
      if (input.type === "checkbox") input.checked = v === "1";
      else input.value = v;
    }
    input.addEventListener("input", run);
    input.addEventListener("change", run);
  }
  run();
}
const state = (ids) =>
  Object.fromEntries(ids.map((id) => [id.slice(3), $(id).type === "checkbox" ? ($(id).checked ? "1" : "") : $(id).value.trim()]));

function big(cls, ...children) {
  return el("article", { class: `vt-card is-big ${cls}` }, ...children);
}

function pog() {
  const ids = ["vt-verbrauch", "vt-leistung", "vt-14a", "vt-optional"];
  wire(ids, () => {
    remember(state(ids));
    result.replaceChildren();
    const kwh = Number(val("vt-verbrauch") || 0);
    const kw = Number(val("vt-leistung") || 0);
    const sect14a = $("vt-14a").checked;
    if (!kwh && !kw && !sect14a && !$("vt-optional").checked) return;
    // § 29 Abs. 1: no Pflichteinbau at ≤ 6 000 kWh, ≤ 7 kW and no § 14a device.
    const unterPflicht = kwh <= 6000 && kw <= 7 && !sect14a;
    const a = call({
      op: "pog",
      verbrauch_kwh: val("vt-verbrauch") || null,
      leistung_kw: val("vt-leistung") || null,
      sect14a,
      optional: $("vt-optional").checked || unterPflicht,
      stichtag: today(),
    });
    if (a.refused) return result.append(refusal(a.refused));
    const r = a.ok;
    const lv = r.letztverbraucher;
    const nb = r.netzbetreiber;
    const headline = lv.eur != null ? fill(UI.you_pay, { eur: eur(lv.eur) }) : lv.angemessen ? UI.angemessen : UI.before;
    result.append(big("is-yes",
      el("p", { class: "vt-due" }, headline),
      r.band ? el("p", { class: "ps-note" }, UI.bands[r.band]) : el("p", { class: "ps-note" }, unterPflicht && !$("vt-optional").checked ? UI.below_pflicht : UI.optional_note),
      nb.eur != null && el("p", {}, fill(UI.nb_pays, { eur: eur(nb.eur) })),
      $("vt-14a").checked && el("p", { class: "ps-note" }, fill(UI.steuer, { eur: eur(r.steuereinrichtung_eur) })),
      el("p", { class: "ps-note" }, UI.source),
    ));
  });
}

function eeg() {
  const ids = ["vt-bauform", "vt-kwp", "vt-ibn", "vt-voll", "vt-einspeisung"];
  if (!$("vt-ibn").value) $("vt-ibn").value = today();
  wire(ids, () => {
    remember(state(ids));
    $("vt-voll").closest("label").hidden = $("vt-bauform").value !== "solar_aufdach";
    result.replaceChildren();
    if (!val("vt-kwp")) return;
    const a = call({
      op: "eeg",
      art: $("vt-bauform").value,
      leistung_kw: val("vt-kwp"),
      inbetriebnahme: val("vt-ibn") || today(),
      voll: $("vt-voll").checked && $("vt-bauform").value === "solar_aufdach",
    });
    if (a.refused) return result.append(refusal(a.refused));
    const r = a.ok;
    if (r.aw_ct == null) {
      return result.append(big("is-no", el("p", { class: "vt-due" }, UI.no_aw), el("p", { class: "ps-note" }, UI.no_aw_hint)));
    }
    const kwh = Number(val("vt-einspeisung"));
    result.append(big("is-yes",
      el("p", { class: "vt-due" }, fill(UI.verguetung, { ct: num(r.verguetung_ct) })),
      el("p", {}, fill(UI.aw, { aw: num(r.aw_ct), abzug: num(r.abzug_ct) })),
      kwh > 0 && el("p", { class: "vt-meaning" }, fill(UI.pro_jahr, { eur: eur((kwh * Number(r.verguetung_ct)) / 100), kwh: num(kwh, 0) })),
      el("p", {}, fill(UI.bis, { datum: new Intl.DateTimeFormat(LANG).format(new Date(r.foerderende)) })),
      el("p", { class: "ps-note" }, UI.source),
    ));
  });
}

function preise() {
  const radios = [...document.querySelectorAll('input[name="vt-gruppe"]')];
  const run = () => {
    const g = radios.find((r) => r.checked)?.value ?? "A";
    remember({ gruppe: g === "A" ? "" : g });
    result.replaceChildren();
    const a = call({ op: "preise", gruppe: g, von: 2026, bis: 2027 });
    if (a.refused) return result.append(refusal(a.refused));
    const [alt, neu] = a.ok.jahre;
    const rows = [["sect19", "sect19_quelle"], ["offshore", "offshore_quelle"], ["kwkg", "kwkg_quelle"], ["stromsteuer", null]];
    const cell = (v) => (v == null ? "—" : `${num(v, 3)} ct`);
    const delta = (a, b) => {
      if (a == null || b == null) return el("td", {}, "");
      const d = Number(b) - Number(a);
      return el("td", { class: d > 0 ? "vt-up" : d < 0 ? "vt-down" : "" }, `${d > 0 ? "+" : ""}${num(d, 3)} ct`);
    };
    let sumA = 0, sumN = 0;
    const body = rows.map(([k, q]) => {
      sumA += Number(alt[k] ?? 0);
      sumN += Number(neu[k] ?? 0);
      return el("tr", {},
        el("th", { scope: "row" }, UI.parts[k]),
        el("td", {}, cell(alt[k])), el("td", {}, cell(neu[k])), delta(alt[k], neu[k]));
    });
    result.append(
      el("table", { class: "vt-table" },
        el("thead", {}, el("tr", {}, el("th", {}, ""), el("th", {}, "2026"), el("th", {}, "2027"), el("th", {}, UI.change))),
        el("tbody", {}, body),
        el("tfoot", {}, el("tr", {}, el("th", { scope: "row" }, UI.sum), el("td", {}, cell(sumA)), el("td", {}, cell(sumN)), delta(sumA, sumN))),
      ),
      el("p", { class: "ps-note" }, fill(UI.ust, { satz: neu.ust_prozent ?? "—" }), " ", UI.not_included),
      el("p", { class: "ps-note" }, UI.sources),
      el("ul", { class: "vt-sources" }, [...new Set(rows.flatMap(([, q]) => (q ? [alt[q], neu[q]] : [])).filter(Boolean))].map((s) => el("li", {}, s))),
    );
  };
  const g = asked().get("gruppe");
  if (g) radios.forEach((r) => (r.checked = r.value === g));
  radios.forEach((r) => r.addEventListener("change", run));
  run();
}

function modul14a() {
  const ids = ["vt-ap", "vt-geraet", "vt-pauschale"];
  wire(ids, () => {
    remember(state(ids));
    result.replaceChildren();
    if (!val("vt-ap") || !val("vt-geraet") || !val("vt-pauschale")) return;
    const a = call({ op: "modul14a", arbeitspreis_ct: val("vt-ap"), geraet_kwh: val("vt-geraet"), pauschale_eur: val("vt-pauschale") });
    if (a.refused) return result.append(refusal(a.refused));
    const r = a.ok;
    const m1 = Number(r.modul1_eur);
    const m2 = Number(r.modul2_eur);
    const max = Math.max(m1, m2, 1);
    const bar = (label, v, win) =>
      el("div", { class: `vt-bar ${win ? "is-win" : ""}` },
        el("span", {}, label),
        el("span", { class: "vt-bar-track" }, el("span", { class: "vt-bar-fill", style: `width:${Math.round((v / max) * 100)}%` })),
        el("strong", {}, eur(v)));
    result.append(big(m2 > m1 ? "is-yes" : "",
      el("p", { class: "vt-due" }, m2 > m1 ? UI.modul2_wins : m1 > m2 ? UI.modul1_wins : UI.tie),
      bar(UI.modul1, m1, m1 >= m2),
      bar(fill(UI.modul2, { p: r.anteil_prozent }), m2, m2 > m1),
      el("p", {}, fill(UI.schwelle, { kwh: num(r.schwelle_kwh, 0) })),
      el("p", { class: "ps-note" }, UI.notes),
    ));
  });
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
    ({ codes, fristen, ids, pog, eeg, preise, modul14a })[root.dataset.tool]();
  })
  .catch((err) => {
    status.textContent = `${UI.load_failed} (${err.message})`;
  });
