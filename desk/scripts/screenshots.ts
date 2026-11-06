// Writes the Desk's screenshots for the docs site into
// site/content/docs/guide/desk/, each in light and dark.
//
//   just desk-screenshots
//
// Deterministic by construction: Vite serves the Desk, makod's MCP answers are
// recorded responses in the schema makod declares (services/makod/mcp.snapshot.json),
// the browser clock is fixed, the zone is Europe/Berlin. No service has to run.

import { mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { type BrowserContext, chromium, type Page } from "playwright";
import { createServer } from "vite";

const desk = resolve(import.meta.dirname, "..");
const out = resolve(desk, "../site/content/docs/guide/desk");
const NOW = "2026-11-04T09:20:00Z"; // 10:20 in Berlin

const KEY = "51238696012";
const UNKNOWN = "51238696782";

// Two processes under one MaLo, with the deadline labels their workflows declare.
const processes = {
  business_key: KEY,
  processes: [
    {
      stream_id: "process/9900357000004/2f6c1e4a-0b5d-4c1e-9a77-3d2b8f1c6e10",
      workflow_name: "gpke-anfrage-bestellung",
      process_id: "2f6c1e4a-0b5d-4c1e-9a77-3d2b8f1c6e10",
      deadlines: [
        {
          deadline_id: "d-ab-1",
          label: "gpke-anfrage-bestellung-antwort",
          art: "prozess",
          due_at: "2026-11-07T10:00:00Z",
        },
      ],
    },
    {
      stream_id: "process/9900357000004/43991093-7780-467a-a6e6-38701f9587ef",
      workflow_name: "gpke-supplier-change",
      process_id: "43991093-7780-467a-a6e6-38701f9587ef",
      deadlines: [
        {
          deadline_id: "d-sc-1",
          label: "gpke-antwortfrist",
          art: "prozess",
          due_at: "2026-11-05T10:00:00Z",
        },
        {
          deadline_id: "d-sc-2",
          label: "aperak-strom-45min-window",
          art: "aperak",
          due_at: "2026-11-04T09:50:00Z",
        },
      ],
    },
  ],
};

// One customer with three Lieferstellen in different states, as vertragd
// declares them (services/vertragd/mcp.snapshot.json).
const stelle = (
  malo_id: string,
  standort: string,
  status: string,
  lieferbeginn: string,
  lieferende: string | null,
) => ({
  malo_id,
  sparte: "STROM",
  vertrags_nr: `VV-2026-${malo_id.slice(0, 4)}`,
  standort,
  lieferbeginn,
  lieferende,
  status,
});
const erika = {
  id: "7c1e2b0a-4d6f-4e3a-9b8c-2f5d1a6e9c30",
  kunden_nr: "K-10042",
  kundentyp: "B2C",
  haushaltskunde: true,
  name: "Erika Mustermann",
  lieferstellen: [
    stelle(KEY, "Wohnung Lindenstraße 4", "ANGEMELDET", "2026-12-01", null),
    stelle("41203768152", "Ferienhaus Kühlungsborn", "AKTIV", "2025-04-01", null),
    stelle("30871225644", "Altbau Gartenweg", "BEENDET", "2021-07-01", "2026-01-31"),
  ],
  lieferstellen_gesamt: 3,
};

// Who supplies which MaLo, as vertragd's `kunden_zu_malos` names them.
const MAX = "62358177402";
const BAECKEREI = "51238696020";
const kunden: Record<string, { kunden_id: string; name: string; kunden_nr: string }> = {
  [KEY]: { kunden_id: erika.id, name: "Erika Mustermann", kunden_nr: "K-10042" },
  [MAX]: {
    kunden_id: "1b9f6c3e-8a2d-4f71-b5e0-6c4a9d2e7f18",
    name: "Max Mustermann",
    kunden_nr: "K-10187",
  },
  [BAECKEREI]: {
    kunden_id: "e4a7d2c9-5f3b-4a81-9c6e-0b2f7d1a8e53",
    name: "Musterbäckerei Ostsee GmbH",
    kunden_nr: "K-20311",
  },
};
const treffer = [
  {
    id: erika.id,
    kunden_nr: "K-10042",
    kundentyp: "B2C",
    name: "Erika Mustermann",
    lieferstellen: 3,
  },
  {
    id: "1b9f6c3e-8a2d-4f71-b5e0-6c4a9d2e7f18",
    kunden_nr: "K-10187",
    kundentyp: "B2C",
    name: "Max Mustermann",
    lieferstellen: 1,
  },
  {
    id: "e4a7d2c9-5f3b-4a81-9c6e-0b2f7d1a8e53",
    kunden_nr: "K-20311",
    kundentyp: "B2B_SLP",
    name: "Musterbäckerei Ostsee GmbH",
    lieferstellen: 2,
  },
];

function vertragdAnswer(message: {
  method: string;
  params?: {
    name?: string;
    arguments?: { query?: string; malos?: string[]; ohne_prozess?: boolean };
  };
}) {
  if (message.method !== "tools/call") return {};
  if (message.params?.name === "kunden_zu_malos") {
    const malos = message.params.arguments?.malos ?? [];
    return {
      content: [],
      structuredContent: {
        kunden: malos.flatMap((malo_id) => {
          const k = kunden[malo_id];
          return k ? [{ malo_id, ...k, sparte: "STROM", status: "ANGEMELDET" }] : [];
        }),
      },
    };
  }
  if (message.params?.name === "list_anmeldungen") {
    const anmeldung = (
      name: string,
      kunden_nr: string,
      malo_id: string,
      mako_process_id: string | null,
    ) => ({
      kunden_id: erika.id,
      name,
      kunden_nr,
      malo_id,
      sparte: "STROM",
      vertrags_nr: `VV-2026-${malo_id.slice(0, 4)}`,
      mako_process_id,
    });
    return {
      content: [],
      structuredContent: {
        anmeldungen: [anmeldung("Musterbäckerei Ostsee GmbH", "K-20311", "40719432813", null)],
        // Three carried by a process (Heute's first list) and this one.
        gesamt: message.params?.arguments?.ohne_prozess ? 1 : 4,
      },
    };
  }
  if (message.params?.name === "search_kunden") {
    return {
      content: [],
      structuredContent: { query: message.params.arguments?.query ?? "", treffer, mehr: false },
    };
  }
  return { content: [], structuredContent: erika };
}

// What each process recorded, as the workflow stated its messages
// (makod `get_process_history`).
const histories: Record<string, unknown> = {
  "43991093-7780-467a-a6e6-38701f9587ef": {
    process_id: "43991093-7780-467a-a6e6-38701f9587ef",
    workflow_name: "gpke-supplier-change",
    eintraege: [
      {
        sequence: 1,
        recorded_at: "2026-11-03T10:12:00Z",
        event_type: "SupplierChangeInitiated",
        richtung: "eingang",
        pid: 55001,
        message_ref: "UTILMD-2026-11-03-0042",
      },
      {
        sequence: 2,
        recorded_at: "2026-11-03T10:12:01Z",
        event_type: "SupplierChangeValidationPassed",
        richtung: null,
        pid: null,
        message_ref: null,
      },
    ],
  },
  "2f6c1e4a-0b5d-4c1e-9a77-3d2b8f1c6e10": {
    process_id: "2f6c1e4a-0b5d-4c1e-9a77-3d2b8f1c6e10",
    workflow_name: "gpke-anfrage-bestellung",
    eintraege: [
      {
        sequence: 1,
        recorded_at: "2026-11-02T14:30:00Z",
        event_type: "AnfrageBestellungInitiated",
        richtung: null,
        pid: null,
        message_ref: null,
      },
    ],
  },
};

function answer(message: {
  id?: number;
  method: string;
  params?: { name?: string; arguments?: { business_key?: string; process_id?: string } };
}) {
  if (message.method !== "tools/call") return {};
  if (message.params?.name === "get_process_history") {
    return {
      content: [],
      structuredContent: histories[message.params.arguments?.process_id ?? ""],
    };
  }
  if (message.params?.arguments?.business_key === KEY) {
    return { content: [], structuredContent: processes };
  }
  return {
    isError: true,
    content: [
      {
        type: "text",
        text: `process_not_found: No process for business key '${message.params?.arguments?.business_key}'.`,
      },
    ],
  };
}

async function recordedVertragd(context: BrowserContext) {
  await context.route("**/svc/vertragd/mcp", async (route) => {
    const message = JSON.parse(route.request().postData() ?? "{}");
    if (message.id === undefined) return route.fulfill({ status: 202 });
    await route.fulfill({
      contentType: "application/json",
      headers: { "mcp-session-id": "screenshots" },
      body: JSON.stringify({ jsonrpc: "2.0", id: message.id, result: vertragdAnswer(message) }),
    });
  });
}

async function recordedMakod(context: BrowserContext) {
  await context.route("**/svc/makod/mcp", async (route) => {
    const message = JSON.parse(route.request().postData() ?? "{}");
    if (message.id === undefined) return route.fulfill({ status: 202 });
    await route.fulfill({
      contentType: "application/json",
      headers: { "mcp-session-id": "screenshots" },
      body: JSON.stringify({ jsonrpc: "2.0", id: message.id, result: answer(message) }),
    });
  });
}

// processd's read model: open processes past their Antwortfrist, each with the
// Fundstelle `mako_fristen` resolved it from.
const overdue = [
  {
    malo_id: "41203768152",
    pid: 55001,
    deadline_at: "2026-11-02T10:00:00Z",
    source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
  },
  {
    malo_id: "30871225644",
    pid: 55007,
    deadline_at: "2026-11-04T04:00:00Z",
    source: "BK6-24-174 GPKE Teil 2, SD Lieferende von LF an NB Prozessschritte 2/3",
  },
].map((o, i) => ({
  process_id: `5a6f0c1e-0000-4000-8000-00000000000${i}`,
  pid: o.pid,
  family: "gpke",
  state: "running",
  malo_id: o.malo_id,
  partner_mp_id: null,
  started_at: "2026-10-28T08:00:00Z",
  deadline_at: o.deadline_at,
  deadline_source: o.source,
}));

// processd's open Anmeldungen, the earliest Frist first, as Heute pages them.
const offen = [
  { malo_id: MAX, deadline_at: "2026-11-04T10:00:00Z" },
  { malo_id: KEY, deadline_at: "2026-11-05T10:00:00Z" },
  { malo_id: BAECKEREI, deadline_at: "2026-11-09T10:00:00Z" },
].map((o, i) => ({
  process_id: `0d1c7a52-0000-4000-8000-00000000000${i}`,
  pid: 55001,
  family: "gpke",
  state: "running",
  malo_id: o.malo_id,
  partner_mp_id: null,
  started_at: "2026-11-03T08:00:00Z",
  deadline_at: o.deadline_at,
  deadline_source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
}));

// A Freigabe the NB's Prozessüberwachung held for a person: an Anmeldung of
// an affiliated supplier, answered by the code its own tree defines.
const freigaben = [
  {
    id: "7b2e4c1a-9f3d-4e8b-a1c2-5d6e7f8a9b0c",
    process_id: "43991093-7780-467a-a6e6-38701f9587ef",
    pid: 55001,
    malo_id: KEY,
    marktrolle: "NB",
    grund:
      "Anmeldung eines verbundenen Lieferanten — zur Gleichbehandlung von einer Person freizugeben",
    art: "eskaliert",
    antwort: null,
    codes_freigeben: {
      ebd: "E_0623",
      codes: [{ code: "A51", bedeutung: "Zustimmung, Lieferbeginn wie angefragt" }],
    },
    codes_ablehnen: {
      ebd: "E_0622",
      codes: [
        { code: "A01", bedeutung: "Marktlokation nicht identifizierbar" },
        { code: "A02", bedeutung: "Marktlokation nimmt nicht an der Marktkommunikation teil" },
      ],
    },
    kann_freigeben: true,
    kann_ablehnen: true,
    status: "Pending",
    expires_at: "2026-11-05T09:00:00Z",
    created_at: "2026-11-04T07:40:00Z",
    decided_by: null,
    decided_at: null,
  },
];

function processdAnswer(message: {
  method: string;
  params?: { name?: string; arguments?: { malos?: string[] } };
}) {
  if (message.method !== "tools/call") return {};
  if (message.params?.name === "list_overdue_processes") {
    return {
      content: [],
      structuredContent: { overdue, count: overdue.length, saturated: false, limit: 500 },
    };
  }
  if (message.params?.name === "list_pending_approvals") {
    return {
      content: [],
      structuredContent: { freigaben, count: freigaben.length, saturated: false },
    };
  }
  if (message.params?.name === "count_open_processes") {
    // processd counts against its own clock; the fixture's is NOW, and the
    // Berlin day ends at 23:00 UTC in November.
    const heute = offen.filter(
      (p) => p.deadline_at >= NOW && p.deadline_at < `${NOW.slice(0, 10)}T23:00:00Z`,
    );
    return {
      content: [],
      structuredContent: {
        offen: offen.length,
        heute_faellig: heute.length,
        heute_frueheste: heute[0]?.deadline_at ?? null,
      },
    };
  }
  const malos = message.params?.arguments?.malos ?? [];
  const prozesse = malos.length ? offen.filter((p) => malos.includes(p.malo_id)) : offen;
  return { content: [], structuredContent: { prozesse, weiter: null } };
}

async function recordedProcessd(context: BrowserContext) {
  await context.route("**/svc/processd/mcp", async (route) => {
    const message = JSON.parse(route.request().postData() ?? "{}");
    if (message.id === undefined) return route.fulfill({ status: 202 });
    const result = processdAnswer(message);
    await route.fulfill({
      contentType: "application/json",
      headers: { "mcp-session-id": "screenshots" },
      body: JSON.stringify({ jsonrpc: "2.0", id: message.id, result }),
    });
  });
}

// The assistant's answer about Erika: two sentences the cited reads carry, one
// with a figure the model worked out — marked, as agentd's check marks it.
// agentd's catalog, generated from the manifests themselves
// (`katalog.fixture.json`): six skills, one not activated.
const katalog = JSON.parse(
  readFileSync(resolve(import.meta.dirname, "katalog.fixture.json"), "utf8"),
);

async function recordedAgentd(context: BrowserContext) {
  // What two skills left for a person: the deadline triage's own sentence,
  // and a model-written row that opens with its disclosure.
  await context.route("**/svc/agentd/api/v1/hinweise", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify([
        {
          task: "t-1",
          run: "r-1",
          art: "deadline.breach",
          skill: "deadline-alert-agent",
          titel: "Fristen-Wächter",
          text: "1 Marktprozess nach der Frist. Die Frist ist abgelaufen; diese Zeile meldet, was versäumt wurde, und hält nichts an.",
          prioritaet: "urgent",
          faellig: "2026-11-06T16:00:00Z",
          anker: [{ art: "malo", wert: "62358177402" }],
        },
        {
          task: "t-2",
          run: "r-2",
          art: "vertrag.anmeldung",
          skill: "vertragd-agent",
          titel: "Verträge und Kunden",
          text: "AI-generated. Die Anmeldung der Lieferstelle 51238696012 wartet noch auf die Antwort des Netzbetreibers.",
          prioritaet: "normal",
          faellig: null,
          anker: [{ art: "malo", wert: "51238696012" }],
        },
      ]),
    }),
  );
  await context.route("**/svc/agentd/api/v1/agents/catalog", (route) =>
    route.fulfill({ contentType: "application/json", body: JSON.stringify(katalog) }),
  );
  await context.route("**/svc/agentd/api/v1/runs", (route) => {
    const input = (route.request().postDataJSON() as { input?: { freigabe?: string } }).input;
    const run_id = input?.freigabe ? "run-freigabe-1" : "run-desk-1";
    return route.fulfill({
      status: 202,
      contentType: "application/json",
      body: JSON.stringify([
        { agent_name: "desk-assistant", run_id, fresh: true, retryable: false },
      ]),
    });
  });
  await context.route("**/svc/agentd/api/v1/runs/*/antwort", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        run: "run-desk-1",
        status: "succeeded",
        disclosure: "AI-generated.",
        saetze: [
          {
            text: "Offen ist die Anmeldung der Lieferstelle 51238696012, Wohnung Lindenstraße 4.",
            quelle: "vertragd/get_kunde",
            belegt: true,
          },
          {
            text: "Der Netzbetreiber muss bis zum 05.11.2026, 11:00 Uhr antworten.",
            quelle: "processd/list_open_processes",
            belegt: true,
          },
          {
            text: "Das sind noch rund 25 Stunden.",
            quelle: "processd/list_open_processes",
            belegt: false,
          },
        ],
        schritte: [
          { quelle: "vertragd/get_kunde", fertig: true },
          { quelle: "makod/get_process", fertig: true },
          { quelle: "processd/list_open_processes", fertig: true },
        ],
        hilfreich: null,
      }),
    }),
  );
  // Registered last: Playwright asks the newest route first.
  await context.route("**/svc/agentd/api/v1/runs/run-freigabe-1/antwort", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        run: "run-freigabe-1",
        status: "succeeded",
        disclosure: "AI-generated.",
        saetze: [
          {
            text: `Die Marktlokation ${KEY} ist bekannt, die Anmeldung kam am 04.11.2026 an.`,
            quelle: "makod/get_process_history",
            belegt: true,
          },
          {
            text: "Ein Ablehnungsgrund aus E_0622 liegt in den gelesenen Daten nicht vor.",
            quelle: "processd/get_queue_entry",
            belegt: true,
          },
        ],
        vorschlag: {
          aktion: "freigeben",
          antwortcode: "A51",
          begruendung:
            "Kein Prüfschritt der Vorprüfung schlägt an; die Anmeldung kann freigegeben werden.",
          quelle: "processd/get_queue_entry",
          belegt: true,
        },
        schritte: [
          { quelle: "processd/get_queue_entry", fertig: true },
          { quelle: "makod/get_process_history", fertig: true },
        ],
        hilfreich: null,
      }),
    }),
  );
}

async function shot(page: Page, name: string, theme: string) {
  await page.waitForTimeout(150); // let transitions settle
  await page.screenshot({ path: resolve(out, `${name}-${theme}.png`) });
}

mkdirSync(out, { recursive: true });
const server = await createServer({ root: desk, server: { port: 0 }, logLevel: "error" });
await server.listen();
const base = server.resolvedUrls?.local[0] ?? "http://localhost:5173/";
const browser = await chromium.launch();

try {
  for (const theme of ["light", "dark"] as const) {
    const context = await browser.newContext({
      colorScheme: theme,
      viewport: { width: 1280, height: 800 },
      deviceScaleFactor: 2,
      locale: "de-DE",
      timezoneId: "Europe/Berlin",
    });
    await recordedMakod(context);
    await recordedVertragd(context);
    await recordedProcessd(context);
    await recordedAgentd(context);
    const page = await context.newPage();
    await page.clock.install({ time: new Date(NOW) });
    await page.addInitScript(() => localStorage.setItem("mako-desk.rolle", "netz"));

    await page.goto(`${base}prozess?key=${KEY}`);
    await page.getByText("2 Prozesse").waitFor();
    await shot(page, "prozesse", theme);

    // ⌘K on a page with a Fragen panel offers to ask the assistant.
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type("Warum wartet die Anmeldung?");
    await page.getByText("Den Assistenten fragen").waitFor();
    await shot(page, "palette-fragen", theme);
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.getByRole("dialog", { name: "Befehle" }).waitFor({ state: "hidden" });

    await page.goto(`${base}prozess`);
    await page.getByText("Welcher Prozess?").waitFor();
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type(KEY);
    await page.getByText(`Prozess ${KEY} öffnen`).waitFor();
    await shot(page, "palette", theme);
    // The first Escape clears the search field, the second closes the palette.
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.getByRole("dialog", { name: "Befehle" }).waitFor({ state: "hidden" });

    await page.getByRole("button", { name: /Rolle: / }).click();
    await page.getByRole("menu", { name: "Rolle wechseln" }).waitFor();
    await shot(page, "rolle", theme);
    await page.keyboard.press("Escape");

    await page.getByRole("button", { name: /Konto und Erscheinungsbild/ }).click();
    await page.getByRole("menu", { name: "Konto" }).waitFor();
    await shot(page, "konto", theme);
    await page.keyboard.press("Escape");

    await page.keyboard.press("ControlOrMeta+/");
    await page.getByRole("dialog", { name: "Tastaturkürzel" }).waitFor();
    await shot(page, "tastatur", theme);
    await page.keyboard.press("Escape");

    await page.goto(`${base}prozess?key=${UNKNOWN}`);
    await page.getByText(`Kein Prozess für „${UNKNOWN}“`).waitFor();
    await shot(page, "refusal", theme);

    // A Freigabe, the assistant's proposal taken into the form — unsent.
    await page.goto(`${base}freigaben`);
    await page.getByRole("button", { name: "Was schlägst du vor?" }).click();
    await page.getByRole("button", { name: "In die Entscheidung übernehmen" }).click();
    await page.getByText(/Vom Assistenten vorgeschlagen/).waitFor();
    await page.getByRole("button", { name: "Freigeben …" }).click();
    await page.getByRole("button", { name: "Jetzt senden" }).waitFor();
    await shot(page, "freigaben", theme);

    // The Lieferant starts from Heute and from the customer.
    await page.addInitScript(() => localStorage.setItem("mako-desk.rolle", "lieferant"));
    await page.goto(`${base}heute`);
    await page.getByRole("heading", { name: /Noch nicht übermittelt/ }).waitFor();
    await page
      .getByText(/heute ab/)
      .first()
      .waitFor();
    await page.getByText("Max Mustermann").waitFor();
    await shot(page, "heute", theme);

    await page.goto(`${base}ueberblick`);
    await page.getByRole("link", { name: "Anmeldungen offen: 4" }).waitFor();
    await shot(page, "ueberblick", theme);

    await page.goto(`${base}fristen`);
    await page.getByRole("link", { name: "41203768152" }).waitFor();
    await shot(page, "fristen", theme);

    await page.goto(`${base}assistent`);
    await page.getByText("Der Assistent liest nur.").waitFor();
    await shot(page, "assistent", theme);

    await page.goto(`${base}kunden?q=Muster`);
    await page.getByText("Musterbäckerei Ostsee GmbH").waitFor();
    await shot(page, "kunden", theme);

    await page.goto(`${base}kunde/${erika.id}`);
    await page.getByRole("heading", { name: "Erika Mustermann", level: 1 }).waitFor();
    await shot(page, "kunde", theme);

    await page.getByRole("button", { name: "Was ist bei diesem Kunden offen?" }).click();
    await page.getByText("nicht belegt").waitFor();
    await page.getByRole("region", { name: "Fragen" }).scrollIntoViewIfNeeded();
    await shot(page, "fragen", theme);

    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type("Muster");
    await page.getByRole("menuitem", { name: /Max Mustermann/ }).waitFor();
    await shot(page, "palette-kunden", theme);

    await context.close();
  }

  const context = await browser.newContext({
    viewport: { width: 1280, height: 900 },
    deviceScaleFactor: 2,
    locale: "de-DE",
    timezoneId: "Europe/Berlin",
  });
  // The shell around the gallery reads too (the Freigaben count): the same
  // recorded answers, so no request reaches a real port.
  await recordedMakod(context);
  await recordedVertragd(context);
  await recordedProcessd(context);
  await recordedAgentd(context);
  const page = await context.newPage();
  await page.goto(`${base}dev/gallery`);
  await page.getByText("Dunkel").waitFor();
  await page.waitForTimeout(150);
  await page.screenshot({ path: resolve(out, "gallery.png") });
  await context.close();
} finally {
  await browser.close();
  await server.close();
}
