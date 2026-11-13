// Holds the Einrichtung view's appearance to its blessed baselines: light,
// dark, and a phone's width in both, compared pixel by pixel with a tolerance.
//
//   just desk-vergleich            compare; exit 1 on a change, the renderings beside the baselines
//   just desk-vergleich --bless    write the baselines after a deliberate change
//
// On the stage of buehne.ts (the production build, every service answer
// recorded in desk/fixtures/), the clock fixed, motion reduced, and the fonts
// pinned to Liberation — which `playwright install --with-deps` puts on every
// Linux runner — so a rendering depends on the code, not on the machine. The
// baselines are Linux renderings: on another system `just desk-vergleich`
// runs this script in Playwright's Linux image.
//
// Two images agree when they have the same size and no more than
// ANTEIL of their pixels differ by more than SCHWELLE in any channel —
// enough to absorb anti-aliasing between CPUs, far too little for a changed
// word, colour or spacing. The comparison runs in the browser's canvas, so it
// needs no image library.

import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { platform } from "node:os";
import { resolve } from "node:path";
import { type Browser, chromium, type Page } from "playwright";
import { aufnehmen, buehne, KONTEXT, seite } from "./buehne.ts";

const desk = resolve(import.meta.dirname, "..");
const BASIS = resolve(desk, "vergleich");
const ABWEICHUNG = resolve(BASIS, "abweichung");
const bless = process.argv.includes("--bless");
const sagen = (zeile: string) => process.stdout.write(`${zeile}\n`);

/** A channel difference above this counts the pixel as changed. */
const SCHWELLE = 64;
/** The share of changed pixels a rendering may carry and still agree. */
const ANTEIL = 0.0003;
const JETZT = "2026-11-04T09:20:00Z";

if (platform() !== "linux") {
  console.error(
    "desk-vergleich: the baselines are Linux renderings — run `just desk-vergleich`, which runs this in Playwright's Linux image.",
  );
  process.exit(2);
}

interface Szene {
  name: string;
  breite: number;
  hoehe: number;
  /** The section heading scrolled to the top; none for the view's head. */
  abschnitt?: string;
}

const SZENEN: Szene[] = [
  { name: "einrichtung", breite: KONTEXT.viewport.width, hoehe: KONTEXT.viewport.height },
  {
    name: "einrichtung-vorschlag",
    breite: KONTEXT.viewport.width,
    hoehe: KONTEXT.viewport.height,
    abschnitt: "Vorschlag",
  },
  { name: "telefon-einrichtung", breite: 390, hoehe: 844 },
  { name: "telefon-einrichtung-vorschlag", breite: 390, hoehe: 844, abschnitt: "Vorschlag" },
];

async function aufnahme(browser: Browser, base: string, s: Szene, dunkel: boolean) {
  const context = await browser.newContext({
    ...KONTEXT,
    viewport: { width: s.breite, height: s.hoehe },
    colorScheme: dunkel ? "dark" : "light",
    reducedMotion: "reduce",
    deviceScaleFactor: 1,
  });
  await aufnehmen(context, "bebildert");
  const page: Page = await seite(context);
  await page.clock.setFixedTime(JETZT);
  await page.addInitScript(() => {
    document.addEventListener("DOMContentLoaded", () => {
      const style = document.createElement("style");
      style.textContent =
        ':root{--font-sans:"Liberation Sans" !important;--font-mono:"Liberation Mono" !important}*{caret-color:transparent !important}';
      document.head.append(style);
    });
  });
  await page.goto(`${base}einrichtung`);
  await page.getByRole("list", { name: "Punkte" }).waitFor();
  await page.getByRole("tablist", { name: "Dateien" }).waitFor();
  await page.evaluate(() => document.fonts.ready);
  if (s.abschnitt)
    await page
      .getByRole("heading", { name: s.abschnitt, level: 2 })
      .evaluate((el) => el.scrollIntoView({ block: "start" }));
  const png = await page.screenshot({ fullPage: true, animations: "disabled" });
  await context.close();
  return png;
}

interface Urteil {
  gleich: boolean;
  grund: string;
  diff: string | null;
}

/** Compare two PNGs in a canvas: the share of changed pixels, and an image of where. */
async function vergleichen(browser: Browser, ist: Buffer, soll: Buffer): Promise<Urteil> {
  const page = await browser.newPage();
  const urteil = await page.evaluate(
    async ({ a, b, schwelle, anteil }) => {
      const bild = async (src: string) => {
        const img = new Image();
        img.src = src;
        await img.decode();
        const c = document.createElement("canvas");
        c.width = img.width;
        c.height = img.height;
        const ctx = c.getContext("2d") as CanvasRenderingContext2D;
        ctx.drawImage(img, 0, 0);
        return ctx.getImageData(0, 0, img.width, img.height);
      };
      const [x, y] = await Promise.all([bild(a), bild(b)]);
      if (x.width !== y.width || x.height !== y.height)
        return {
          gleich: false,
          grund: `size ${x.width}×${x.height}, baseline ${y.width}×${y.height}`,
          diff: null,
        };
      const out = new ImageData(x.width, x.height);
      let anders = 0;
      const ist = (i: number) => x.data[i] ?? 0;
      const soll = (i: number) => y.data[i] ?? 0;
      for (let i = 0; i < x.data.length; i += 4) {
        const d = Math.max(
          Math.abs(ist(i) - soll(i)),
          Math.abs(ist(i + 1) - soll(i + 1)),
          Math.abs(ist(i + 2) - soll(i + 2)),
        );
        const hit = d > schwelle;
        if (hit) anders++;
        out.data[i] = hit ? 255 : ist(i) / 4;
        out.data[i + 1] = hit ? 0 : ist(i + 1) / 4;
        out.data[i + 2] = hit ? 64 : ist(i + 2) / 4;
        out.data[i + 3] = 255;
      }
      const share = anders / (x.width * x.height);
      const c = document.createElement("canvas");
      c.width = x.width;
      c.height = x.height;
      (c.getContext("2d") as CanvasRenderingContext2D).putImageData(out, 0, 0);
      return {
        gleich: share <= anteil,
        grund: `${anders} pixels (${(share * 100).toFixed(3)} %) differ, ${(anteil * 100).toFixed(3)} % allowed`,
        diff: c.toDataURL("image/png"),
      };
    },
    {
      a: `data:image/png;base64,${ist.toString("base64")}`,
      b: `data:image/png;base64,${soll.toString("base64")}`,
      schwelle: SCHWELLE,
      anteil: ANTEIL,
    },
  );
  await page.close();
  return urteil;
}

const { base, schliessen } = await buehne();
const browser = await chromium.launch();
let fehler = 0;
try {
  mkdirSync(BASIS, { recursive: true });
  rmSync(ABWEICHUNG, { recursive: true, force: true });
  for (const s of SZENEN) {
    for (const dunkel of [false, true]) {
      const name = `${s.name}-${dunkel ? "dark" : "light"}`;
      const datei = resolve(BASIS, `${name}.png`);
      const ist = await aufnahme(browser, base, s, dunkel);
      // The same scene twice: a rendering that is not stable on one machine
      // cannot be held to a baseline.
      const nochmal = await aufnahme(browser, base, s, dunkel);
      const stabil = await vergleichen(browser, ist, nochmal);
      if (!stabil.gleich) {
        console.error(
          `✗ ${name}: two renderings disagree (${stabil.grund}) — the scene is not deterministic`,
        );
        fehler++;
        continue;
      }
      if (bless) {
        writeFileSync(datei, ist);
        sagen(`✓ ${name}: blessed`);
        continue;
      }
      if (!existsSync(datei)) {
        console.error(`✗ ${name}: no baseline — run \`just desk-vergleich --bless\``);
        fehler++;
        continue;
      }
      const u = await vergleichen(browser, ist, readFileSync(datei));
      if (u.gleich) {
        sagen(`✓ ${name}: ${u.grund}`);
        continue;
      }
      fehler++;
      mkdirSync(ABWEICHUNG, { recursive: true });
      writeFileSync(resolve(ABWEICHUNG, `${name}.ist.png`), ist);
      if (u.diff)
        writeFileSync(
          resolve(ABWEICHUNG, `${name}.diff.png`),
          Buffer.from(u.diff.replace(/^data:image\/png;base64,/, ""), "base64"),
        );
      console.error(
        `✗ ${name}: ${u.grund} — see desk/vergleich/abweichung/; a deliberate change is blessed with \`just desk-vergleich --bless\``,
      );
    }
  }
} finally {
  await browser.close();
  await schliessen();
}
process.exit(fehler === 0 ? 0 : 1);
