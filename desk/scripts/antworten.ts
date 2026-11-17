// The recorded service answers the Desk's browser checks and `npm run
// dev:fixtures` both run on: every typed tool answers in the shape its
// mcp.snapshot.json declares, makod's checklist and a rehearsal's verdict are
// desk/fixtures/, agentd's runs are the recorded streams (`aufzeichnung.ts`).
// No service has to run.

import { readFileSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { resolve } from "node:path";
import { lesen } from "../src/test-support/aufzeichnung.ts";

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
  fakten_freigeben: [],
  fakten_ablehnen: [],
  kann_freigeben: true,
  kann_ablehnen: true,
  status: "Pending",
  expires_at: "2026-11-05T10:00:00Z",
  created_at: "2026-11-04T08:00:00Z",
  decided_by: null,
  decided_at: null,
};

/** makod's checklist and a rehearsal's verdict, as makod answered them (desk/fixtures/). */
const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(resolve(FIXTURES, name), "utf8"));
const PROBE = fixture("probe-44004-abgelehnt.json");
/** agentd's answer to the Einrichtung's pre-fill run (desk/fixtures/, held to agentd). */
export const VORBELEGT = fixture("vorbelegen.json");

// What each typed tool answers, by name; any other read is a process's Verlauf, empty.
const ANTWORTEN: Record<string, unknown> = {
  einrichtung_checkliste: fixture("einrichtung-checkliste.json"),
  einrichtung_vorschlag: fixture("einrichtung-vorschlag.json"),
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

/** One request to a service door, as the stage sees it. */
export interface Anfrage {
  pathname: string;
  method: string;
  body: string | null;
}

/** One recorded answer. */
export interface Antwort {
  status: number;
  contentType?: string;
  headers?: Record<string, string>;
  body?: string;
}

/**
 * The recorded answer to `anfrage`; a question starts the recorded run `run`,
 * whose stream `aufzeichnungen()` plays. A door the stage does not know is
 * taken (204).
 */
export function beantworten(anfrage: Anfrage, run: string): Antwort {
  const { pathname } = anfrage;
  const json = (body: unknown, status = 200): Antwort => ({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
  if (pathname.endsWith("/mcp")) {
    const m = JSON.parse(anfrage.body ?? "{}");
    if (m.id === undefined) return { status: 202 };
    const name = m.method === "tools/call" ? (m.params?.name as string) : null;
    const result = name ? { content: [], structuredContent: ANTWORTEN[name] ?? VERLAUF } : {};
    return {
      status: 200,
      contentType: "application/json",
      headers: { "mcp-session-id": "buehne" },
      body: JSON.stringify({ jsonrpc: "2.0", id: m.id, result }),
    };
  }
  if (pathname.startsWith("/svc/makod/api/v1/probe/")) return json(PROBE);
  if (pathname === "/svc/agentd/api/v1/agents/catalog")
    return { status: 200, contentType: "application/json", body: katalog };
  if (
    pathname === "/svc/agentd/api/v1/runs" &&
    anfrage.method === "POST" &&
    JSON.parse(anfrage.body ?? "{}").agent === "einrichtung-assistent"
  )
    return json(
      [
        {
          agent_name: "einrichtung-assistent",
          run_id: "vorbelegen",
          fresh: true,
          retryable: false,
        },
      ],
      202,
    );
  if (pathname.endsWith("/vorbelegen/antwort")) return json(VORBELEGT);
  if (pathname === "/svc/agentd/api/v1/runs" && anfrage.method === "POST")
    return json(
      [{ agent_name: "desk-assistant", run_id: run, fresh: true, retryable: false }],
      202,
    );
  if (pathname.endsWith("/antwort")) return json(antwortAus(run));
  if (pathname.endsWith("/stopp")) return { status: 202 };
  if (pathname.startsWith("/svc/agentd/api/v1/vorschlaege")) return json({});
  if (pathname.startsWith("/svc/agentd/")) return json([]);
  // A decision's door: taken. The checks end each page before a hold ends.
  return { status: 204 };
}

/** A Connect middleware answering every `/svc/` door but a run's stream from the recording. */
export function fixturen(run = "beantwortet") {
  return (req: IncomingMessage, res: ServerResponse, next: () => void) => {
    const pathname = (req.url ?? "").split("?")[0] ?? "";
    if (!pathname.startsWith("/svc/") || pathname.endsWith("/schritte")) return next();
    const teile: Buffer[] = [];
    req.on("data", (c: Buffer) => teile.push(c));
    req.on("end", () => {
      const a = beantworten(
        { pathname, method: req.method ?? "GET", body: Buffer.concat(teile).toString("utf8") },
        run,
      );
      res.writeHead(a.status, {
        ...(a.contentType ? { "content-type": a.contentType } : {}),
        ...a.headers,
      });
      res.end(a.body);
    });
  };
}
