// Holds every route's accessibility tree to a blessed baseline — Playwright's
// ARIA snapshot of `main`, the view itself without the sidebar or the top bar:
//
//   just desk-aria            compare; exit 1 on a change, the diff printed
//   just desk-aria --bless    write the baselines after a deliberate change
//
// What a screen reader is given — the roles, names and levels of a view — is
// the part of a change a pixel diff is blind to and a reviewer rarely reads:
// a heading demoted, a button that lost its name, a list that became divs.
// On the stage of buehne.ts (the production build, every service answer
// recorded), the clock fixed, so a baseline depends on the code alone. The
// tree is text, so unlike the pixel baselines it is the same on every OS.
// `locator.ariaSnapshot()` is what @playwright/test's `toMatchAriaSnapshot`
// compares; the Desk runs Playwright as a library and compares it itself.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { aufnehmen, buehne, KONTEXT, seite } from "./buehne.ts";

const BASIS = resolve(import.meta.dirname, "../aria");
const bless = process.argv.includes("--bless");
const JETZT = "2026-11-04T09:20:00Z";
const sagen = (zeile: string) => process.stdout.write(`${zeile}\n`);

/** Every route, with the level-1 heading that says its view has rendered. */
const ROUTEN: { name: string; pfad: string; titel: RegExp }[] = [
  { name: "heute", pfad: "heute", titel: /^Heute$/ },
  { name: "freigaben", pfad: "freigaben", titel: /^Freigaben$/ },
  { name: "fristen", pfad: "fristen", titel: /^Fristen/ },
  { name: "ueberblick", pfad: "ueberblick", titel: /^Überblick$/ },
  { name: "dead-letters", pfad: "dead-letters", titel: /^Dead Letters$/ },
  { name: "prozesse", pfad: "prozess", titel: /^Prozesse$/ },
  { name: "prozess", pfad: "prozess?key=51238696012", titel: /^Lieferantenwechsel/ },
  { name: "kunden", pfad: "kunden", titel: /^Kunden$/ },
  { name: "assistent", pfad: "assistent", titel: /^Assistent$/ },
  { name: "schattenbetrieb", pfad: "schattenbetrieb", titel: /^Schattenbetrieb$/ },
  { name: "einrichtung", pfad: "einrichtung", titel: /^Einrichtung$/ },
  { name: "lokation", pfad: "lokation/51238696012", titel: /^Marktlokation 51238696012/ },
  { name: "nachrechnen", pfad: "rechnung/b-1", titel: /^Nachrechnen$/ },
  { name: "gegenpartei", pfad: "gegenpartei/9800357000005/NB", titel: /Gasnetz|9800357000005/ },
  { name: "gegenparteien", pfad: "gegenparteien", titel: /^Gegenparteien$/ },
  { name: "zustellungen", pfad: "zustellungen", titel: /^Zustellungen$/ },
];

/** The lines of `a` and `b` that differ, marked as a unified diff marks them. */
function unterschied(a: string, b: string): string[] {
  const alt = a.split("\n");
  const neu = b.split("\n");
  const out: string[] = [];
  for (let i = 0; i < Math.max(alt.length, neu.length); i++) {
    if (alt[i] === neu[i]) continue;
    if (alt[i] !== undefined) out.push(`- ${alt[i]}`);
    if (neu[i] !== undefined) out.push(`+ ${neu[i]}`);
  }
  return out;
}

const stage = await buehne();
const browser = await chromium.launch();
let abweichend = 0;
try {
  mkdirSync(BASIS, { recursive: true });
  for (const r of ROUTEN) {
    const context = await browser.newContext({ ...KONTEXT, reducedMotion: "reduce" });
    await aufnehmen(context, "beantwortet");
    const page = await seite(context);
    await page.clock.setFixedTime(JETZT);
    await page.goto(`${stage.base}${r.pfad}`);
    await page.getByRole("heading", { level: 1, name: r.titel }).waitFor();
    await page.waitForLoadState("networkidle");
    // Skeletons gone: every read the view started has answered.
    await page.locator("main [aria-busy='true']").waitFor({ state: "detached" });
    const baum = `${await page.locator("main").ariaSnapshot()}\n`;
    const datei = resolve(BASIS, `${r.name}.yml`);
    if (bless || !existsSync(datei)) {
      writeFileSync(datei, baum);
      sagen(`aria: ${r.name} written`);
    } else {
      const diff = unterschied(readFileSync(datei, "utf8"), baum);
      if (diff.length) {
        abweichend++;
        sagen(`aria: ${r.name} differs:`);
        for (const z of diff) sagen(`  ${z}`);
      } else {
        sagen(`aria: ${r.name} unchanged`);
      }
    }
    await context.close();
  }
} finally {
  await browser.close();
  await stage.schliessen();
}
if (abweichend) {
  sagen(`aria: ${abweichend} route(s) differ — after a deliberate change: just desk-aria --bless`);
}
process.exit(abweichend ? 1 : 0);
