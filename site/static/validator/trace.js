// The landing page's trace: a sample or a dropped file travels the six
// stages of the path, and each stage shows what the validator module computes
// for it. The module loads on the first click, from the Validator's URL, so
// the two pages share one cached download.

import { MAX, el, load, readFile } from "./module.js";

const root = document.querySelector(".trace");
if (root) start(root);

function start(root) {
  const UI = JSON.parse(document.getElementById("trace-ui").textContent);
  const LANG = root.dataset.lang === "de" ? "de-DE" : "en-GB";
  const status = root.querySelector(".trace-status");
  const open = root.querySelector(".trace-open");
  const stages = Object.fromEntries(
    [...root.querySelectorAll(".trace-stage")].map((s) => [s.dataset.stage, s]),
  );
  const fill = (s, vars) => s.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");
  const due = new Intl.DateTimeFormat(LANG, {
    timeZone: "Europe/Berlin", weekday: "short", day: "2-digit", month: "2-digit",
    hour: "2-digit", minute: "2-digit",
  });
  // A source cited at length („… — refines this into …") shows its citation.
  const cite = (q) => q.split(" — ")[0];
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;

  let call = null;
  let loading = null;
  const ready = () =>
    (loading ??= (status.textContent = UI.loading, load(root.dataset.wasm)).then((c) => {
      call = c;
      status.textContent = "";
      if (!c) throw new Error("WebAssembly");
      return c;
    }));

  let run = 0;
  async function trace(text, sample) {
    const mine = ++run;
    try {
      await ready();
    } catch (err) {
      status.textContent = `${UI.failed} (${err.message})`;
      return;
    }
    const answer = call({ op: "check", text, empfangen: new Date().toISOString() });
    open.href = root.dataset.validator + (sample ? `#beispiel=${encodeURIComponent(sample)}` : "");
    for (const s of Object.values(stages)) {
      s.classList.remove("is-on", "is-bad");
      s.querySelector(".trace-live").replaceChildren();
    }
    if (answer.refused || answer.ok.nachrichten[0].fehler) {
      status.textContent = UI.not_edifact;
      return;
    }
    status.textContent = "";
    const m = answer.ok.nachrichten[0];
    const lines = view(m);
    for (const [i, key] of ["in", "check", "route", "frist", "answer", "out"].entries()) {
      if (!still) await new Promise((r) => setTimeout(r, i === 0 ? 0 : 260));
      if (mine !== run) return; // a newer message took over
      const stage = stages[key];
      stage.querySelector(".trace-live").replaceChildren(...(lines[key] ?? []));
      stage.classList.add("is-on");
      if (key === "check" && !m.gueltig) stage.classList.add("is-bad");
    }
  }

  /** What each stage says about `m`: elements, every value as text. */
  function view(m) {
    const errors = m.befunde.filter((b) => b.schwere === "fehler").length;
    const a = m.antwort;
    const contrl = (m.fristen ?? []).find((f) => f.was === "CONTRL");
    return {
      in: [el("span", { class: "trace-mono" }, `${m.absender ?? "?"} → ${m.empfaenger ?? "?"}`), el("span", {}, `${m.typ ?? ""} ${m.formatversion ?? ""}`)],
      check: m.gueltig
        ? [el("span", {}, `AHB ${m.ahb ?? "?"} · ${fill(UI.segments, { n: m.segmente.length })}`), el("span", { class: "trace-ok" }, `✓ ${UI.no_findings}`)]
        : [el("span", { class: "trace-err" }, `✗ ${fill(UI.errors, { n: errors || m.befunde.length })}`), el("span", {}, m.befunde[0]?.text ?? "")],
      route: [
        el("span", {}, `PID ${m.pid ?? "—"}${m.anwendungsfall ? ` · ${m.anwendungsfall}` : ""}`),
        m.route
          ? el("span", { class: "trace-mono" }, `${m.route.crate} · ${m.route.workflow}`)
          : el("span", {}, UI.no_route),
      ],
      frist: a && a.faellig
        ? [
            el("strong", {}, fill(UI.answers_by, { who: a.durch, when: due.format(new Date(a.faellig)) })),
            a.entscheidungsbaum && el("span", {}, fill(UI.decided_by, { ebd: a.entscheidungsbaum })),
            el("span", { class: "trace-src" }, cite(a.quelle)),
          ]
        : contrl
          ? [el("strong", {}, fill(UI.contrl_by, { when: due.format(new Date(contrl.faellig)) })), el("span", { class: "trace-src" }, contrl.quelle)]
          : [],
      answer: [
        m.aperak && el("strong", {}, UI[`aperak_${m.aperak.art}`]),
        m.aperak && el("span", { class: "trace-src" }, m.aperak.quelle),
        a && el("span", {}, `${UI.then} ${a.pids[0]} / ${a.pids[1]}`),
      ],
      out: [],
    };
  }

  for (const chip of root.querySelectorAll("[data-sample]")) {
    chip.addEventListener("click", async () => {
      try {
        await ready();
      } catch (err) {
        status.textContent = `${UI.failed} (${err.message})`;
        return;
      }
      const s = call({ op: "sample", name: chip.dataset.sample });
      if (s.ok) trace(s.ok.text, chip.dataset.sample);
    });
  }
  root.querySelector(".trace-file input").addEventListener("change", async (e) => {
    const file = e.target.files[0];
    e.target.value = "";
    if (!file) return;
    try {
      const text = await readFile(file, UI.too_big);
      if (text.length > MAX) throw new Error(UI.too_big);
      trace(text, null);
    } catch (err) {
      status.textContent = err.message;
    }
  });
}
