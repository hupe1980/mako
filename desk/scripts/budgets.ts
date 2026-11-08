// The Desk's interaction budgets, measured in a browser — the first streamed
// step within 1 s, ⌘K's actions and a keyboard decision within 100 ms:
//
//   just desk-budgets                      # every budget, 20 runs each
//   node scripts/budgets.ts --runs 40      # more runs
//   node scripts/budgets.ts --run verspaetet   # the first step from another recording
//
// Playwright drives the production build under `vite preview` (scripts/buehne.ts):
// agentd's stream is a recorded run from desk/fixtures/, played with its
// delays over a real event stream, and every other service answer is
// recorded. Each budget holds at the 95th percentile of its runs, and every
// measurement is printed.
//
// - **Erster Schritt** — from the key that sends a question to the first
//   streamed step of the trail on screen: ≤ 1000 ms.
// - **Aktionen** — from choosing the entry ⌘K found to its actions on
//   screen: ≤ 100 ms.
// - **Entscheidung per Tastatur** — the Interaction to Next Paint of a
//   decision taken from the keyboard, in its view (R, then ↵ on the send) and
//   in ⌘K (↵ in its parameters): ≤ 100 ms. INP is read from the browser's
//   Event Timing entries (`PerformanceObserver`, type `event`), the longest
//   interaction of the decision.
//
// The run proves its own teeth before it measures: `run-verspaetet.jsonl`
// sends its first step 1500 ms late, and a run of the first budget against it
// must fail, naming the interaction. A budget that cannot fail measures
// nothing.

import { parseArgs } from "node:util";
import { type Browser, chromium, type Page } from "playwright";
import { aufnehmen, buehne, KONTEXT, seite } from "./buehne.ts";

/** A line for the reader; every measurement and finding is printed. */
const sagen = (zeile: string) => process.stdout.write(`${zeile}\n`);

const { values } = parseArgs({
  options: {
    runs: { type: "string", default: "20" },
    run: { type: "string", default: "beantwortet" },
  },
});
const RUNS = Number.parseInt(values.runs, 10);
const RUN = values.run;

/** One budget: what is measured, its ceiling, and how one run measures it. */
interface Budget {
  name: string;
  grenzeMs: number;
  messen: (page: Page, base: string) => Promise<number>;
}

/** The 95th percentile, nearest rank. */
export function p95(werte: number[]): number {
  const s = [...werte].sort((a, b) => a - b);
  return s[Math.max(0, Math.ceil(0.95 * s.length) - 1)] ?? Number.NaN;
}

/** The budget's verdict on its runs: null when it holds, else the sentence naming it. */
export function urteil(name: string, grenzeMs: number, werte: number[]): string | null {
  const p = p95(werte);
  return p <= grenzeMs
    ? null
    : `${name}: p95 ${p.toFixed(0)} ms über dem Budget von ${grenzeMs} ms`;
}

// In the page: the start of the next interaction of `art`, and the frame in
// which `selektor` first stands on screen — the next animation frame after it
// entered the DOM, which is the frame that paints it.
async function markieren(page: Page, art: "keydown" | "pointerdown", selektor: string) {
  await page.evaluate(
    ({ art, selektor }) => {
      const w = window as unknown as { __mark: { t0?: number; t1?: number } };
      w.__mark = {};
      document.addEventListener(
        art,
        (e) => {
          if (
            w.__mark.t0 === undefined &&
            (art !== "keydown" || (e as KeyboardEvent).key === "Enter")
          )
            w.__mark.t0 = e.timeStamp;
        },
        { capture: true },
      );
      const da = () => {
        requestAnimationFrame(() => {
          w.__mark.t1 = performance.now();
        });
      };
      if (document.querySelector(selektor)) return da();
      const obs = new MutationObserver(() => {
        if (!document.querySelector(selektor)) return;
        obs.disconnect();
        da();
      });
      obs.observe(document.body, { childList: true, subtree: true });
    },
    { art, selektor },
  );
}

async function gemessen(page: Page): Promise<number> {
  await page.waitForFunction(
    () => (window as unknown as { __mark: { t1?: number } }).__mark.t1 !== undefined,
    undefined,
    { timeout: 15_000 },
  );
  return page.evaluate(() => {
    const m = (window as unknown as { __mark: { t0?: number; t1: number } }).__mark;
    if (m.t0 === undefined) throw new Error("keine Eingabe gemessen");
    return m.t1 - m.t0;
  });
}

// INP: every Event Timing entry from here on, grouped by interaction; the
// decision's INP is its longest interaction. An interaction under the
// observer's 16 ms floor reports no entry and counts as 16 ms.
async function inpBeobachten(page: Page) {
  await page.evaluate(() => {
    const w = window as unknown as { __inp: Map<number, number> };
    w.__inp = new Map();
    new PerformanceObserver((liste) => {
      for (const e of liste.getEntries() as PerformanceEventTiming[]) {
        if (!e.interactionId) continue;
        w.__inp.set(e.interactionId, Math.max(w.__inp.get(e.interactionId) ?? 0, e.duration));
      }
    }).observe({ type: "event", durationThreshold: 16 } as PerformanceObserverInit);
  });
}

async function inp(page: Page): Promise<number> {
  // Event Timing reports an entry after its next paint; give it two frames.
  await page.evaluate(
    () => new Promise<void>((ok) => requestAnimationFrame(() => requestAnimationFrame(() => ok()))),
  );
  await page.waitForTimeout(100);
  return page.evaluate(() => {
    const w = window as unknown as { __inp: Map<number, number> };
    return Math.max(16, ...w.__inp.values());
  });
}

/** ⌘Z: the hold taken back, so closing the tab sends nothing anywhere. */
async function zuruecknehmen(page: Page) {
  await page.keyboard.press("ControlOrMeta+z");
  await page.getByRole("timer").first().waitFor({ state: "detached" });
}

async function zuFreigabe(page: Page, base: string) {
  await page.goto(`${base}freigaben?id=q-1`);
  await page.getByRole("heading", { name: /verbundenen Lieferanten/ }).waitFor();
}

async function paletteAufFreigabe(page: Page, base: string) {
  await page.goto(`${base}heute`);
  await page.getByRole("button", { name: /verbundenen Lieferanten/ }).waitFor();
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("q-1");
  await page.getByRole("menuitem", { name: /Freigabe/ }).waitFor();
}

const ersterSchritt: Budget = {
  name: "Erster gestreamter Schritt nach dem Senden der Frage",
  grenzeMs: 1000,
  async messen(page, base) {
    await zuFreigabe(page, base);
    const feld = page.getByRole("textbox", { name: "Frage an den Assistenten" });
    await feld.fill("Warum wartet die Anmeldung?");
    await markieren(page, "keydown", "[data-schritt]");
    await feld.press("Enter");
    return gemessen(page);
  },
};

const aktionenSichtbar: Budget = {
  name: "Aktionen eines in ⌘K gefundenen Eintrags sichtbar",
  grenzeMs: 100,
  async messen(page, base) {
    await paletteAufFreigabe(page, base);
    await markieren(page, "pointerdown", '[role="listbox"][aria-label="Aktionen"]');
    await page.getByRole("menuitem", { name: /Freigabe/ }).click();
    return gemessen(page);
  },
};

const entscheidungAnsicht: Budget = {
  name: "Entscheidung per Tastatur in ihrer Ansicht (INP)",
  grenzeMs: 100,
  async messen(page, base) {
    await zuFreigabe(page, base);
    await inpBeobachten(page);
    await page.keyboard.press("r");
    await page.locator("label").filter({ hasText: "A02" }).click();
    await page.getByRole("button", { name: "Ablehnen", exact: true }).focus();
    await page.keyboard.press("Enter");
    await page
      .getByRole("status")
      .getByText(/Freigabe abgelehnt/)
      .waitFor();
    // The pointer chose the code; INP is the keyboard's here: R and ↵.
    const ms = await inp(page);
    await zuruecknehmen(page);
    return ms;
  },
};

const entscheidungPalette: Budget = {
  name: "Entscheidung per Tastatur in ⌘K (INP)",
  grenzeMs: 100,
  async messen(page, base) {
    await paletteAufFreigabe(page, base);
    await page.getByRole("menuitem", { name: /Freigabe/ }).click();
    await page.getByRole("option", { name: /^Ablehnen/ }).click();
    await page.locator("label").filter({ hasText: "A02" }).click();
    await inpBeobachten(page);
    // ↵ from the chosen code, where the focus is.
    await page.keyboard.press("Enter");
    await page
      .getByRole("timer", { name: /Gleich gesendet/ })
      .first()
      .waitFor();
    const ms = await inp(page);
    await zuruecknehmen(page);
    return ms;
  },
};

/** Run `budget` `runs` times, each in a new tab, printing every measurement. */
async function laufen(
  browser: Browser,
  base: string,
  run: string,
  budget: Budget,
  runs: number,
): Promise<number[]> {
  const context = await browser.newContext(KONTEXT);
  await aufnehmen(context, run);
  const werte: number[] = [];
  try {
    for (let i = 0; i < runs; i++) {
      const page = await seite(context);
      werte.push(await budget.messen(page, base));
      await page.close();
    }
  } finally {
    await context.close();
  }
  sagen(`${budget.name} (Budget ${budget.grenzeMs} ms, p95 über ${runs} Läufe)`);
  sagen(`  ${werte.map((w) => w.toFixed(0)).join(" ")} ms`);
  sagen(`  p95 ${p95(werte).toFixed(0)} ms`);
  return werte;
}

const stage = await buehne();
const browser = await chromium.launch();
const verfehlt: string[] = [];
try {
  // The teeth first: a first step sent 1500 ms late must fail, by name.
  const spaet = await laufen(browser, stage.base, "verspaetet", ersterSchritt, 3);
  const probe = urteil(ersterSchritt.name, ersterSchritt.grenzeMs, spaet);
  if (!probe?.startsWith(ersterSchritt.name)) {
    verfehlt.push(
      `Selbstprobe: run-verspaetet.jsonl hielt das Budget „${ersterSchritt.name}" — es kann nicht fehlschlagen`,
    );
  } else {
    sagen(`  Selbstprobe: ${probe} — wie erwartet\n`);
  }

  for (const budget of [
    ersterSchritt,
    aktionenSichtbar,
    entscheidungAnsicht,
    entscheidungPalette,
  ]) {
    const werte = await laufen(browser, stage.base, RUN, budget, RUNS);
    const u = urteil(budget.name, budget.grenzeMs, werte);
    sagen(u ? `  VERFEHLT: ${u}\n` : "  gehalten\n");
    if (u) verfehlt.push(u);
  }
} finally {
  await browser.close();
  await stage.schliessen();
}

if (verfehlt.length) {
  for (const v of verfehlt) console.error(`desk-budgets: ${v}`);
  process.exit(1);
}
sagen("desk-budgets: jedes Budget gehalten");
