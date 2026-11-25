// `just desk-axe` checks keyboard navigation and WCAG 2.2 A/AA findings in both themes.
// Reduced motion and focus visibility under the sticky header need explicit browser checks.

import AxeBuilder from "@axe-core/playwright";
import { type Browser, chromium, type Locator, type Page } from "playwright";
import { aufnehmen, buehne, KONTEXT, seite } from "./buehne.ts";

/** A line for the reader; every measurement and finding is printed. */
const sagen = (zeile: string) => process.stdout.write(`${zeile}\n`);

const fehler: string[] = [];

/** From the top of the page, Tab until `ziel` has the focus; refused after 80 stops. */
async function tabZu(page: Page, ziel: Locator, was: string): Promise<void> {
  const handle = await ziel.elementHandle();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let i = 0; i < 80; i++) {
    await page.keyboard.press("Tab");
    if (await page.evaluate((el) => el === document.activeElement, handle)) return;
  }
  throw new Error(`${was}: mit Tab nicht erreichbar`);
}

/** In ⌘K's list, ↓ until the item named `name` is the one ↵ runs, then ↵. */
async function waehlen(page: Page, name: RegExp): Promise<void> {
  const item = page.getByRole("menuitem", { name });
  await item.waitFor();
  for (let i = 0; i < 12; i++) {
    if ((await item.getAttribute("data-focused")) !== null) return page.keyboard.press("Enter");
    await page.keyboard.press("ArrowDown");
  }
  throw new Error(`⌘K: „${name.source}" mit den Pfeiltasten nicht erreichbar`);
}

/**
 * Scan the surface `bereich` as it stands, and refuse an animation running
 * anywhere on the page. The scan stays on the surface it names.
 */
async function pruefen(page: Page, zustand: string, bereich: string): Promise<void> {
  const laufend = await page.evaluate(() =>
    document
      .getAnimations()
      .filter((a) => a.playState === "running")
      .map((a) => {
        const ziel = (a.effect as KeyframeEffect | null)?.target as Element | null;
        return `${(a as CSSAnimation).animationName ?? a.constructor.name} auf ${ziel?.className ?? "?"}`;
      }),
  );
  for (const l of laufend)
    fehler.push(`${zustand}: Animation läuft trotz reduzierter Bewegung — ${l}`);
  const ergebnis = await new AxeBuilder({ page })
    .include(bereich)
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
    .analyze();
  const schwer = ergebnis.violations.filter(
    (v) => v.impact === "serious" || v.impact === "critical",
  );
  for (const v of ergebnis.violations) {
    const wo = v.nodes.map((n) => n.target.join(" ")).join(", ");
    sagen(`  ${v.impact ?? "?"} ${v.id}: ${v.help} — ${wo}`);
  }
  for (const v of schwer) fehler.push(`${zustand}: axe ${v.impact} ${v.id} — ${v.help}`);
  sagen(
    `${zustand}: ${ergebnis.passes.length} Regeln bestanden, ${ergebnis.violations.length} Befunde, ${laufend.length} laufende Animationen`,
  );
}

/**
 * Scroll `ziel` up under the top bar, focus it, and refuse it if the focus
 * leaves it there: its top must end at or below the bar's bottom. In a short
 * window, so the view has room to scroll it that far.
 */
async function nichtVerdeckt(page: Page, ziel: Locator, zustand: string): Promise<void> {
  const vorher = page.viewportSize();
  await page.setViewportSize({ width: vorher?.width ?? 1280, height: 240 });
  const lage = await ziel.evaluate((el) => {
    const main = el.closest("main");
    const bar = main?.querySelector("header");
    if (!main || !bar) return null;
    // The citation's top a few pixels below the view's top, under the bar.
    main.scrollTop += el.getBoundingClientRect().top - main.getBoundingClientRect().top - 4;
    const unter = el.getBoundingClientRect().top < bar.getBoundingClientRect().bottom;
    (el as HTMLElement).focus();
    return {
      unter,
      oben: el.getBoundingClientRect().top,
      barUnten: bar.getBoundingClientRect().bottom,
    };
  });
  if (vorher) await page.setViewportSize(vorher);
  if (!lage) return void fehler.push(`${zustand}: keine Ansicht mit Kopfleiste gefunden`);
  if (!lage.unter)
    return void fehler.push(`${zustand}: der Beleg ließ sich nicht unter die Kopfleiste scrollen`);
  if (lage.oben < lage.barUnten)
    fehler.push(
      `${zustand}: der fokussierte Beleg bleibt unter der Kopfleiste (oben ${lage.oben.toFixed(0)} px, Leiste bis ${lage.barUnten.toFixed(0)} px)`,
    );
  else
    sagen(
      `${zustand}: der fokussierte Beleg steht unterhalb der Kopfleiste, nicht von ihr verdeckt`,
    );
}

async function frageStellen(page: Page, base: string) {
  await page.goto(`${base}freigaben?id=q-1`);
  await page.getByRole("heading", { name: /verbundenen Lieferanten/ }).waitFor();
  // ⌘J puts the focus in the question; ↵ sends it.
  await page.keyboard.press("ControlOrMeta+j");
  await page.keyboard.type("Warum wartet die Anmeldung?");
  await page.keyboard.press("Enter");
}

async function durchgang(browser: Browser, base: string, theme: "light" | "dark") {
  const neu = async (
    run: string,
    viewport: { width: number; height: number } = KONTEXT.viewport,
  ) => {
    const context = await browser.newContext({
      ...KONTEXT,
      viewport,
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    await aufnehmen(context, run);
    return { context, page: await seite(context) };
  };
  const zustand = (name: string) => `${name} (${theme === "light" ? "hell" : "dunkel"})`;

  for (const language of ["de", "en"] as const) {
    const de = language === "de";
    const { context, page } = await neu("shadow");
    await page.addInitScript((s) => localStorage.setItem("mako-desk.sprache", s), language);
    await page.goto(`${base}schattenbetrieb`);
    await page.getByText(de ? /1 von 2 Eingängen geprüft/ : /1 of 2 inbound assessed/).waitFor();
    const row = page.getByRole("row", {
      name: de ? /Eingang.*Abweichung/ : /Occurrence.*Difference/,
    });
    await row.focus();
    await page.keyboard.press("Enter");
    const code = page.getByRole("row", {
      name: de ? /Antwortcode A02 A07/ : /Answer code A02 A07/,
    });
    await code.waitFor();
    await page.waitForFunction(
      () =>
        document.activeElement?.closest("section[tabindex]") !== null &&
        document.activeElement?.closest("section[tabindex]") !== undefined,
    );
    await page.keyboard.press("Escape");
    await page.waitForFunction(
      () =>
        document.activeElement?.closest('[role="row"]')?.getAttribute("aria-selected") === "true",
    );
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page
      .getByText(de ? /Für diesen Eingang fehlt/ : /This occurrence needs a comparison/)
      .waitFor();
    await page.getByRole("button", { name: de ? "Erneut prüfen" : "Recheck" }).click();
    await page
      .getByRole("status")
      .filter({ hasText: de ? "Aktueller Stand geladen." : "Current evidence loaded." })
      .waitFor();
    await pruefen(page, zustand(`Shadow review ${language}`), "main");
    await context.close();
  }

  // The same MaLo can owe two suppliers a period. The run evidence stays
  // keyboard accessible at phone width while both issuer rows remain distinct.
  {
    const de = theme === "light";
    const context = await browser.newContext({
      ...KONTEXT,
      viewport: { width: 390, height: 844 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    await aufnehmen(context, "beantwortet");
    await context.addInitScript(
      (language) => {
        localStorage.setItem("mako-desk.rolle", "lieferant");
        localStorage.setItem("mako-desk.sprache", language);
      },
      de ? "de" : "en",
    );
    const page = await context.newPage();
    await page.goto(`${base}heute?quelle=abrechnungen`);
    await page
      .getByRole("status")
      .filter({
        hasText: de ? "Ermittelte Vertragskomponenten: 2" : "Contract components found: 2",
      })
      .waitFor();
    for (const issuer of ["9900357000004", "9900357000028"]) {
      await page.getByRole("button", { name: new RegExp(`Lieferant ${issuer}`) }).waitFor();
    }
    const details = page.locator("summary").filter({ hasText: de ? "Laufdetails" : "Run details" });
    await tabZu(page, details, "Billing discovery");
    await page.keyboard.press("Enter");
    await page.getByText(de ? /Nächster geplanter Versuch:/ : /Next scheduled attempt:/).waitFor();
    await pruefen(page, zustand("Billing discovery"), "main");
    await page.goto(`${base}rechnung/b-1`);
    await page
      .getByRole("status")
      .filter({ hasText: de ? /kein Nachweis/ : /No evidence/ })
      .waitFor();
    await page.getByText(de ? /Versand unterdrückt/ : /sending suppressed/).waitFor();
    await pruefen(page, zustand("Invoice delivery evidence"), "main");
    await context.close();
  }

  // The LF keyboard path starts in Heute and ends at the refusal's diagnostic.
  {
    const de = theme === "light";
    const language = de ? "de" : "en";
    const context = await browser.newContext({
      ...KONTEXT,
      viewport: { width: 320, height: 720 },
      reducedMotion: "reduce",
      colorScheme: theme,
    });
    await aufnehmen(context, "klaerfall-lf");
    await context.addInitScript((language) => {
      localStorage.setItem("mako-desk.rolle", "lieferant");
      localStorage.setItem("mako-desk.sprache", language);
    }, language);
    const page = await context.newPage();
    const posts: { key: string | undefined; body: Record<string, unknown> }[] = [];
    await page.route("**/api/v1/klaerfaelle/*/dispatch", async (route) => {
      const request = route.request();
      posts.push({ key: request.headers()["idempotency-key"], body: request.postDataJSON() });
      if (de) return route.fallback();
      return route.fulfill({
        status: 422,
        json: { code: "unknown_case_code", detail: "synthetic diagnostic" },
      });
    });
    await page.goto(`${base}heute?quelle=klaerfaelle`);
    const eintrag = page.getByRole("button", { name: /UTILMD-LF-DEMO/ });
    await tabZu(page, eintrag, "Heute, LF");
    await page.keyboard.press("Enter");
    const cause = page.getByRole("heading", { level: 1, name: de ? /Ursache Z29/ : /Cause Z29/ });
    await cause.waitFor();
    await page
      .getByText(de ? "Zuständige Rolle · Lieferant" : "Responsible Rolle · Lieferant")
      .waitFor();
    await page.getByText(de ? "Frist unbekannt" : "Frist unknown").waitFor();
    const source = page.getByRole("button", { name: /ERC/ });
    await tabZu(page, source, "LF, Quelle");
    await page.keyboard.press("Enter");
    await page.getByRole("dialog").getByText("ERC", { exact: true }).waitFor();
    await page.keyboard.press("Escape");
    const send = page.getByRole("button", { name: de ? "Korrigieren & senden" : "Correct & send" });
    await tabZu(page, send, "LF, Korrektur");
    const overflow = await send.evaluate((el) => {
      const main = el.closest("main");
      // A compressed heading can overlap the action while the page itself still fits.
      const list = [
        document.documentElement,
        main,
        el.closest("section"),
        main?.querySelector("nav"),
        el.closest("section")?.querySelector("h2"),
      ];
      return list.some((node) => node && node.scrollWidth > node.clientWidth);
    });
    if (overflow) throw new Error(`LF (${language}): 320 px requires horizontal scrolling`);
    await page.keyboard.press("Enter");
    await page.getByRole("button", { name: de ? /Rückgängig/ : /Undo/ }).waitFor();
    await page.keyboard.press("ControlOrMeta+z");
    const undone = page.getByRole("status").filter({
      hasText: de ? "Zurückgenommen. Es wurde nichts gesendet." : "Undone. Nothing was sent.",
    });
    await undone.waitFor();
    if (
      (await undone.count()) !== 1 ||
      !(await send.evaluate((el) => el === document.activeElement))
    )
      throw new Error(`LF (${language}): undo announcement or return focus missing`);
    // Flushing the page releases every surviving hold, even before its timer expires.
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await page.reload();
    await cause.waitFor();
    if (posts.length) throw new Error(`LF (${language}): undo sent a request`);
    const radio = page.getByRole("radio", { checked: true });
    await tabZu(page, radio, "LF, Korrektur wählen");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Tab");
    const reason = page.getByRole("textbox", { name: de ? /Begründung/ : /Reason/ });
    if (!(await reason.evaluate((el) => el === document.activeElement)))
      throw new Error("LF: reason lacks focus");
    await page.keyboard.type("Kundenauftrag liegt vor");
    await tabZu(page, send, "LF, Alternative senden");
    await page.keyboard.press("Enter");
    const refusal = page.getByRole("status").filter({
      hasText: de ? "Dieser Klärfall ist schon abgeschlossen." : "The service refused the request.",
    });
    await refusal.waitFor();
    const details = refusal.locator("summary");
    if (
      (await refusal.count()) !== 1 ||
      (de && (await send.count()) !== 0) ||
      !(await details.evaluate((el) => el === document.activeElement)) ||
      (await reason.inputValue()) !== "Kundenauftrag liegt vor" ||
      posts.length !== 1 ||
      posts[0]?.body.befehl !== "gpke.kuendigung.ablehnen" ||
      posts[0]?.body.begruendung !== "Kundenauftrag liegt vor" ||
      !posts[0]?.key?.match(/^[0-9a-f-]{36}$/)
    )
      throw new Error(`LF (${language}): refusal, retained reason, key or focus missing`);
    const raw = refusal.getByText(de ? /case_closed:/ : /unknown_case_code:/);
    if (await raw.isVisible()) throw new Error("LF: service text exposed before Details");
    await page.keyboard.press("Enter");
    await raw.waitFor();
    await pruefen(page, zustand(`LF Klärfall • ${language} • 320 px`), "main");
    await context.close();
  }

  // The trail, while the run reads.
  {
    const { context, page } = await neu("verspaetet");
    await frageStellen(page, base);
    await page.locator("[data-schritt]").first().waitFor();
    await page.getByRole("button", { name: "Stoppen" }).waitFor();
    await pruefen(page, zustand("Spur, live"), 'section[aria-label="Fragen"]');

    // Left while it reads: the pill in the top bar, reached with Tab. ⌘K
    // opens on the selected Freigabe's actions; esc goes back to the search.
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByRole("listbox", { name: "Aktionen" }).waitFor();
    await page.keyboard.press("Escape");
    const suche = page.getByRole("searchbox", { name: "Befehl oder Geschäftsschlüssel" });
    await suche.waitFor();
    await suche.pressSequentially("Überblick", { delay: 20 });
    await waehlen(page, /^Überblick/);
    const pille = page.getByRole("list", { name: "Laufende Fragen" }).getByRole("button");
    await pille.first().waitFor();
    await tabZu(page, pille.first(), zustand("Pille"));
    await pruefen(page, zustand("Pille"), 'ul[aria-label="Laufende Fragen"]');
    await context.close();
  }

  // The receipt, once answered: reached with Tab, ↵ opens the trail again.
  {
    const { context, page } = await neu("beantwortet");
    await frageStellen(page, base);
    const quittung = page.getByRole("button", { name: /Quellen gelesen/ });
    await quittung.waitFor();
    await pruefen(page, zustand("Quittung"), 'section[aria-label="Fragen"]');
    await nichtVerdeckt(
      page,
      page.getByRole("button", { name: /^Beleg / }).first(),
      zustand("Beleg"),
    );
    await tabZu(page, quittung, zustand("Quittung"));
    await page.keyboard.press("Enter");
    await page.getByRole("list", { name: "Was der Assistent gelesen hat" }).waitFor();
    await pruefen(page, zustand("Quittung, geöffnet"), 'section[aria-label="Fragen"]');
    await context.close();
  }

  // The Überblick: its tiles are links named by what they show.
  {
    const { context, page } = await neu("beantwortet");
    await page.goto(`${base}ueberblick`);
    await page.getByRole("link", { name: /^Fristen überschritten/ }).waitFor();
    await pruefen(page, zustand("Überblick"), "main");
    await context.close();
  }

  // The Prozess page: the record, its decision bar reached with Tab and opened
  // with ↵, and the Stand slider moved with the arrow keys.
  {
    const { context, page } = await neu("beantwortet");
    await page.goto(`${base}prozess?key=51238696012`);
    await page.getByRole("list", { name: "Nachweise" }).waitFor();
    await pruefen(page, zustand("Prozess"), "main");
    const entscheiden = page.getByRole("button", { name: "Entscheiden" });
    await tabZu(page, entscheiden, zustand("Prozess, Entscheiden"));
    await page.keyboard.press("Enter");
    await page.getByRole("radiogroup").first().waitFor();
    await pruefen(page, zustand("Prozess, Entscheidung offen"), "main");
    await context.close();
  }

  // Fristen, the list–detail–inspector layout: Tab into the list, ↵ opens the
  // row into the detail, esc returns to it — then the same in English.
  {
    const { context, page } = await neu("beantwortet");
    const ueberfaellig = {
      process_id: "p-1",
      pid: 55001,
      family: "gpke",
      state: "running",
      malo_id: "51238696012",
      partner_mp_id: null,
      started_at: "2026-11-01T08:00:00Z",
      deadline_at: "2026-11-03T10:00:00Z",
      deadline_source: "BK6-24-174 GPKE Teil 2",
    };
    await context.route("**/svc/processd/mcp", async (route) => {
      const m = JSON.parse(route.request().postData() ?? "{}");
      if (m.params?.name !== "list_overdue_processes") return route.fallback();
      return route.fulfill({
        contentType: "application/json",
        headers: { "mcp-session-id": "buehne" },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: m.id,
          result: {
            content: [],
            structuredContent: { overdue: [ueberfaellig], count: 1, saturated: false, limit: 500 },
          },
        }),
      });
    });
    await page.goto(`${base}fristen`);
    const zeile = page.getByRole("row", { name: /51238696012/ });
    await zeile.waitFor();
    await tabZu(page, zeile, zustand("Fristen, Liste"));
    await page.keyboard.press("Enter");
    await page.getByText(/^51238696012 · PID/).waitFor();
    const imDetail = await page
      .waitForFunction(() => document.activeElement?.tagName === "SECTION", null, {
        timeout: 2000,
      })
      .then(() => true)
      .catch(() => false);
    if (!imDetail) {
      const wo = await page.evaluate(() => document.activeElement?.outerHTML.slice(0, 120));
      fehler.push(`${zustand("Fristen")}: ↵ führt den Fokus nicht ins Detail (${wo})`);
    }
    await pruefen(page, zustand("Fristen, Detail"), "main");
    await page.keyboard.press("Escape");
    const zurueck = await zeile.evaluate((el) => el === document.activeElement);
    if (!zurueck) fehler.push(`${zustand("Fristen")}: esc führt den Fokus nicht zur Zeile zurück`);
    else sagen(`${zustand("Fristen")}: ↵ öffnet ins Detail, esc kehrt zur Zeile zurück`);
    await page.evaluate(() => localStorage.setItem("mako-desk.sprache", "en"));
    await page.goto(`${base}fristen?id=p-1`);
    await page.getByRole("heading", { name: "Fristen overdue", level: 1 }).waitFor();
    const lang = await page.evaluate(() => document.documentElement.lang);
    if (lang !== "en") fehler.push(`${zustand("Fristen, English")}: <html lang> ist „${lang}“`);
    await pruefen(page, zustand("Fristen, English"), "main");
    await context.close();
  }

  // Einrichtung: the checklist as makod answered it, then a Probelauf run from
  // the keyboard — Tab into its facts, ⌘↵ — and its verdict and segments.
  {
    const { context, page } = await neu("beantwortet");
    await page.goto(`${base}einrichtung`);
    await page.getByRole("list", { name: "Punkte" }).waitFor();
    await pruefen(page, zustand("Einrichtung, Checkliste"), "main");
    const angaben = page.getByRole("textbox", { name: "Angaben (JSON)" });
    await tabZu(page, angaben, zustand("Probelauf, Angaben"));
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.locator("[data-urteil]").waitFor();
    await pruefen(page, zustand("Einrichtung, Probelauf"), "main");
    await context.close();
  }

  // The Vorschlag: a sentence sent with ↵, the pre-fill's offers reached and
  // taken from the keyboard, then a gap's „Zur Frage" moves focus to its question.
  {
    const { context, page } = await neu("beantwortet");
    await page.goto(`${base}einrichtung`);
    await page.getByRole("tablist", { name: "Dateien" }).waitFor();
    const satz = page.getByRole("textbox", { name: "Aus einem Satz" });
    await tabZu(page, satz, zustand("Vorschlag, Satz"));
    await page.keyboard.type("Wir sind Lieferant Strom und Gas.");
    await page.keyboard.press("Enter");
    await page.getByRole("list", { name: "Vorgeschlagene Antworten" }).waitFor();
    await pruefen(page, zustand("Einrichtung, Vorschlag"), "main");
    const alle = page.getByRole("button", { name: "Alle übernehmen" });
    await tabZu(page, alle, zustand("Vorschlag, Alle übernehmen"));
    await page.keyboard.press("Enter");
    await page.getByRole("textbox", { name: "MP-ID Lieferant Strom" }).waitFor();
    const frage = page.getByRole("button", { name: "Zur Frage" }).first();
    await frage.focus();
    await page.keyboard.press("Enter");
    const gelandet = await page
      .waitForFunction(() => document.activeElement?.closest("[data-frage]") !== null, null, {
        timeout: 2000,
      })
      .then(() => true)
      .catch(() => false);
    if (!gelandet)
      fehler.push(`${zustand("Vorschlag, Zur Frage")}: der Fokus landet auf keiner Frage`);
    await context.close();
  }

  // At a phone's width the sidebar is a sheet: the top bar's button opens it
  // from the keyboard, esc closes it and the focus returns to the button —
  // and no view scrolls sideways.
  {
    const { context, page } = await neu("beantwortet", { width: 390, height: 844 });
    for (const pfad of ["heute", "einrichtung", "fristen", "freigaben", "ueberblick", "prozess"]) {
      await page.goto(`${base}${pfad}`);
      await page.locator("main h1").first().waitFor();
      const breit = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      if (breit > 0) fehler.push(`${zustand(`Telefon, /${pfad}`)}: ${breit} px seitlich zu breit`);
    }
    await page.goto(`${base}einrichtung`);
    await page.getByRole("list", { name: "Punkte" }).waitFor();
    await pruefen(page, zustand("Telefon, Einrichtung"), "main");
    const knopf = page.getByRole("button", { name: "Ansichten öffnen" });
    await tabZu(page, knopf, zustand("Telefon, Menü"));
    await page.keyboard.press("Enter");
    const blatt = page.getByRole("dialog", { name: "Ansichten" });
    await blatt.waitFor();
    await pruefen(page, zustand("Telefon, Ansichten"), '[role="dialog"]');
    await page.keyboard.press("Escape");
    await blatt.waitFor({ state: "detached" });
    const zurueckAmKnopf = await knopf.evaluate(
      (el) =>
        new Promise<boolean>((ok) => {
          const bis = performance.now() + 1000;
          const schauen = () =>
            el === document.activeElement
              ? ok(true)
              : performance.now() > bis
                ? ok(false)
                : requestAnimationFrame(schauen);
          schauen();
        }),
    );
    if (!zurueckAmKnopf)
      fehler.push(`${zustand("Telefon, Menü")}: esc gibt den Fokus nicht an den Knopf zurück`);
    await context.close();
  }

  // The action panel: ⌘K, the identifier, ↵ on the entry, ↵ on its action.
  {
    const { context, page } = await neu("beantwortet");
    await page.goto(`${base}heute`);
    await page.getByRole("button", { name: /verbundenen Lieferanten/ }).waitFor();
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type("q-1");
    await waehlen(page, /Freigabe/);
    await page.getByRole("listbox", { name: "Aktionen" }).waitFor();
    await pruefen(page, zustand("Aktionen"), '[role="dialog"]');
    await page.keyboard.press("Enter");
    await page.getByRole("group", { name: /Parameter/ }).waitFor();
    await pruefen(page, zustand("Aktion, Parameter"), '[role="dialog"]');
    await context.close();
  }
}

const stage = await buehne();
const browser = await chromium.launch();
try {
  for (const theme of ["light", "dark"] as const) {
    try {
      await durchgang(browser, stage.base, theme);
    } catch (e) {
      // The first lines name the step and what it waited for.
      fehler.push(e instanceof Error ? e.message.split("\n").slice(0, 3).join(" ") : String(e));
    }
  }
} finally {
  await browser.close();
  await stage.schliessen();
}

if (fehler.length) {
  for (const f of fehler) console.error(`desk-axe: ${f}`);
  process.exit(1);
}
sagen("desk-axe: jede Fläche zugänglich, in hell und dunkel, ohne Bewegung");
