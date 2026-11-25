// The recorded service answers the Desk's browser checks and `npm run
// dev:fixtures` both run on: every typed tool answers in the shape its
// mcp.snapshot.json declares, makod's checklist and a rehearsal's verdict are
// desk/fixtures/, agentd's runs are the recorded streams (`aufzeichnung.ts`).
// No service has to run.

import { readFileSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { resolve } from "node:path";
import billingWatch from "../fixtures/billing-watch.json" with { type: "json" };
import lfAkte from "../fixtures/klaerfall-lf.json" with { type: "json" };
import shadowReview from "../fixtures/shadow-review.json" with { type: "json" };
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
  list_unbilled_periods: billingWatch,
  summarize_billing_deadlines: {
    discovery: billingWatch.discovery,
    lieferanten: billingWatch.periods.map(({ lf_mp_id }) => ({
      lf_mp_id,
      owed: 1,
      overdue: 0,
      issued_on_time: 0,
      issued_late: 0,
    })),
  },
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
// The process the Freigabe is held on, as makod and processd state it.
const PROZESS = {
  "makod/get_process": {
    business_key: "51238696012",
    processes: [
      {
        stream_id: "process/buehne/p-1",
        workflow_name: "gpke-supplier-change",
        process_id: "p-1",
        deadlines: [
          {
            deadline_id: "d-1",
            label: "gpke-antwortfrist",
            art: "prozess",
            due_at: "2026-11-05T10:00:00Z",
          },
        ],
      },
    ],
  },
  "processd/get_process": {
    process_id: "p-1",
    pid: 55001,
    family: "gpke",
    workflow_name: "gpke-supplier-change",
    state: "running",
    malo_id: "51238696012",
    partner_mp_id: null,
    mdm_role: null,
    deadline_at: "2026-11-05T10:00:00Z",
    deadline_source: null,
    deadline_risk: "ok",
    started_at: "2026-11-04T08:00:00Z",
    last_event_at: "2026-11-04T08:00:00Z",
    erc_code: null,
    antwort_code: null,
    antwort_codeliste: null,
    bedeutung: null,
    initiator_is_affiliate: true,
  },
};
// The Lokation page's REST reads and the MCP answers of the Nachrechnen and
// Gegenpartei views, recorded in the shapes the services declare.
const REST = fixture("lokation.json") as Record<string, unknown>;
const ANSICHTEN = fixture("ansichten.json") as Record<string, unknown>;

const VERLAUF = {
  process_id: "p-1",
  workflow_name: "gpke-supplier-change",
  eintraege: [
    {
      sequence: 1,
      recorded_at: "2026-11-04T08:00:00Z",
      event_type: "SupplierChangeInitiated",
      richtung: "eingang",
      pid: 55001,
      message_ref: "UTILMD-2026-11-04-0001",
      antwortcode: null,
      codeliste: null,
      bedeutung: null,
    },
  ],
};

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
    const dienst = pathname.split("/")[2] ?? "";
    const faelle = run === "klaerfall-lf" && dienst === "processd";
    const fall =
      faelle && name === "list_klaerfaelle"
        ? { klaerfaelle: [lfAkte.klaerfall], count: 1, saturated: false }
        : faelle && name === "get_klaerfall" && m.params?.arguments?.id === lfAkte.klaerfall.id
          ? lfAkte
          : undefined;
    const antwort =
      fall ??
      (PROZESS as Record<string, unknown>)[`${dienst}/${name}`] ??
      (name ? (ANTWORTEN[name] ?? ANSICHTEN[name]) : undefined) ??
      VERLAUF;
    const result = name ? { content: [], structuredContent: antwort } : {};
    return {
      status: 200,
      contentType: "application/json",
      headers: { "mcp-session-id": "buehne" },
      body: JSON.stringify({ jsonrpc: "2.0", id: m.id, result }),
    };
  }
  if (
    run === "klaerfall-lf" &&
    anfrage.method === "POST" &&
    pathname === `/svc/processd/api/v1/klaerfaelle/${lfAkte.klaerfall.id}/dispatch`
  ) {
    return json({ code: "case_closed", detail: "the case has already been closed" }, 409);
  }
  if (pathname === "/svc/outputd/api/v1/documents/by-subject/INVOICE")
    return json({
      kind: "INVOICE",
      subject_ref: "R-2026-0042",
      deliveries: [
        {
          channel: "EMAIL",
          status: "FAILED",
          delivered_at: null,
          read_at: null,
          first_sent_at: null,
        },
        {
          channel: "POST",
          status: "SUPPRESSED",
          delivered_at: null,
          read_at: null,
          first_sent_at: null,
        },
      ],
    });
  if (pathname === "/svc/processd/api/v1/shadow/review") return json(shadowReview);
  if (pathname in REST) return json(REST[pathname]);
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
export function fixturen(run = process.env.MAKO_DESK_FIXTURE ?? "beantwortet") {
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
