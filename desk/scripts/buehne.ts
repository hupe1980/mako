// The stage the Desk's browser checks run on (`budgets.ts`, `zugang.ts`): the
// production build served by `vite preview`, agentd's stream played from
// desk/fixtures/ by the recorded-run middleware (`aufzeichnung.ts`), and every
// other service answer recorded in the shape its mcp.snapshot.json declares.
// No service has to run, and nothing here measures — it only sets the scene.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { BrowserContext, Page } from "playwright";
import { build, preview } from "vite";
import { lesen } from "../src/test-support/aufzeichnung.ts";
import { aufzeichnungen } from "./aufzeichnung.ts";

const desk = resolve(import.meta.dirname, "..");
const FIXTURES = resolve(desk, "fixtures");

/** The Freigabe every check decides: one Anmeldung, the codes its two trees publish. */
export const FREIGABE = {
  id: "q-1",
  process_id: "p-1",
  pid: 55001,
  malo_id: "51238696012",
  marktrolle: "NB",
  grund: "Anmeldung eines verbundenen Lieferanten (Antwortfrist 2026-11-05)",
  art: "eskaliert",
  antwort: null,
  codes_freigeben: { ebd: "E_0623", codes: [{ code: "E15", bedeutung: "Zustimmung" }] },
  codes_ablehnen: {
    ebd: "E_0622",
    codes: [
      { code: "A01", bedeutung: "Marktlokation nicht gefunden" },
      { code: "A02", bedeutung: "nimmt nicht an der Marktkommunikation teil" },
    ],
  },
  kann_freigeben: true,
  kann_ablehnen: true,
  status: "Pending",
  expires_at: "2026-11-05T10:00:00Z",
  created_at: "2026-11-04T08:00:00Z",
  decided_by: null,
  decided_at: null,
};

// What each typed tool answers, by name; any other read is a process's Verlauf, empty.
const ANTWORTEN: Record<string, unknown> = {
  list_pending_approvals: { freigaben: [FREIGABE], count: 1, saturated: false },
  list_disputes: { disputes: [], count: 0, saturated: false },
  list_review_queue: { records: [], count: 0, saturated: false },
  list_parked_bookings: { buchungen: [], count: 0, saturated: false },
  list_open_processes: { prozesse: [], weiter: null },
  list_overdue_processes: { overdue: [], count: 0, saturated: false, limit: 500 },
  list_anmeldungen: { anmeldungen: [], gesamt: 0 },
  list_korrekturbedarf: { korrekturbedarf: [] },
  kunden_zu_malos: { kunden: [] },
  partner_namen: { partner: [] },
};
const VERLAUF = { process_id: "p-1", workflow_name: "gpke-supplier-change", eintraege: [] };

const katalog = readFileSync(resolve(import.meta.dirname, "katalog.fixture.json"), "utf8");

/** The answer a recorded run ends with — what `GET …/antwort` reads where no stream is. */
function antwortAus(run: string): unknown {
  const lines = lesen(readFileSync(resolve(FIXTURES, `run-${run}.jsonl`), "utf8"));
  return lines.find((l) => l.event === "antwort")?.data ?? null;
}

/**
 * Record every service answer in `context`. A question starts the recorded
 * run `run` — its stream is left to the preview server, which plays the
 * fixture with its delays, so the browser waits on a real event stream.
 */
export async function aufnehmen(context: BrowserContext, run: string): Promise<void> {
  const json = (body: unknown, status = 200) => ({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
  await context.route(
    (url) => url.pathname.startsWith("/svc/") && !url.pathname.endsWith("/schritte"),
    async (route) => {
      const req = route.request();
      const { pathname } = new URL(req.url());
      if (pathname.endsWith("/mcp")) {
        const m = JSON.parse(req.postData() ?? "{}");
        if (m.id === undefined) return route.fulfill({ status: 202 });
        const name = m.method === "tools/call" ? (m.params?.name as string) : null;
        const result = name ? { content: [], structuredContent: ANTWORTEN[name] ?? VERLAUF } : {};
        return route.fulfill({
          contentType: "application/json",
          headers: { "mcp-session-id": "buehne" },
          body: JSON.stringify({ jsonrpc: "2.0", id: m.id, result }),
        });
      }
      if (pathname === "/svc/agentd/api/v1/agents/catalog")
        return route.fulfill({ contentType: "application/json", body: katalog });
      if (pathname === "/svc/agentd/api/v1/runs" && req.method() === "POST")
        return route.fulfill(
          json([{ agent_name: "desk-assistant", run_id: run, fresh: true, retryable: false }], 202),
        );
      if (pathname.endsWith("/antwort")) return route.fulfill(json(antwortAus(run)));
      if (pathname.endsWith("/stopp")) return route.fulfill({ status: 202 });
      if (pathname.startsWith("/svc/agentd/api/v1/vorschlaege")) return route.fulfill(json({}));
      if (pathname.startsWith("/svc/agentd/")) return route.fulfill(json([]));
      // A decision's door: taken. The checks end each page before a hold ends.
      return route.fulfill({ status: 204 });
    },
  );
}

/** A new tab on the stage, in the Rolle Netz, its zone and language German. */
export async function seite(context: BrowserContext): Promise<Page> {
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("mako-desk.rolle", "netz"));
  return page;
}

export const KONTEXT = {
  viewport: { width: 1280, height: 800 },
  locale: "de-DE",
  timezoneId: "Europe/Berlin",
} as const;

/** Build the Desk and serve the build, with the recorded runs in front of it. */
export async function buehne(): Promise<{ base: string; schliessen: () => Promise<void> }> {
  await build({ root: desk, logLevel: "error" });
  // The build is the config file's; the preview takes none of it, so no dev
  // proxy stands behind the stage and no request can reach a running service.
  const server = await preview({
    root: desk,
    configFile: false,
    logLevel: "error",
    preview: { port: 0, host: "127.0.0.1" },
    plugins: [
      {
        name: "aufzeichnungen",
        configurePreviewServer(s) {
          s.middlewares.use(aufzeichnungen(FIXTURES));
        },
      },
    ],
  });
  const base = server.resolvedUrls?.local[0];
  if (!base) throw new Error("vite preview hat keine Adresse");
  return {
    base,
    schliessen: () =>
      new Promise<void>((ok, fehler) => server.httpServer.close((e) => (e ? fehler(e) : ok()))),
  };
}
