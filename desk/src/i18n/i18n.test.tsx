import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import checklisteJson from "../../fixtures/einrichtung-checkliste.json?raw";
import vorschlagJson from "../../fixtures/einrichtung-vorschlag.json?raw";
import { DIENST } from "../app/dienste";
import { PROBEN, zeigen } from "../app/proben";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import { de, type Katalog } from "./de";
import { en } from "./en";
import { katalogErsetzen, setSprache } from "./index";

async function open(path: string) {
  const router = createDeskRouter(createMemoryHistory({ initialEntries: [path] }));
  await router.load();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RolleProvider>
        <RouterProvider router={router} />
      </RolleProvider>
    </QueryClientProvider>,
  );
  return router;
}

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  katalogErsetzen(null);
  setSprache("de");
  localStorage.removeItem("mako-desk.rolle");
});

const OVERDUE = [
  {
    process_id: "p-1",
    pid: 55001,
    family: "gpke",
    state: "running",
    malo_id: "51238696012",
    partner_mp_id: "9900357000004",
    started_at: "2026-10-01T08:00:00Z",
    deadline_at: "2026-10-02T09:00:00Z",
    deadline_source: "BK6-24-174 GPKE Teil 2",
  },
  {
    process_id: "p-2",
    pid: 55004,
    family: "gpke",
    state: "antwortfrist_abgelaufen",
    malo_id: "41203768152",
    partner_mp_id: null,
    started_at: null,
    deadline_at: "2026-10-05T09:00:00Z",
    deadline_source: null,
  },
];

const DEAD = [
  {
    rejected_at: "2026-11-07T09:15:00Z",
    reason_label: "unknown_pid",
    reason_detail: "PID 99999 is not registered",
  },
];

/** processd's overdue list, makod's dead letters and a Verlauf, as the services answer. */
function services() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url: string, init: RequestInit) => {
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/call") {
        const name = m.params.name as string;
        if (name === "list_overdue_processes")
          result = {
            structuredContent: { overdue: OVERDUE, count: 2, saturated: false, limit: 500 },
          };
        else if (name === "list_dead_letters")
          result = { structuredContent: { dead_letters: DEAD, gesamt: 1 } };
        else if (name === "get_process_history")
          result = {
            structuredContent: {
              process_id: m.params.arguments.process_id,
              workflow_name: "gpke-supplier-change",
              eintraege: [
                {
                  sequence: 1,
                  recorded_at: "2026-10-01T08:00:00Z",
                  event_type: "SupplierChangeInitiated",
                  richtung: "eingang",
                  pid: 55001,
                  message_ref: "UTILMD-1",
                },
              ],
            },
          };
        else result = { structuredContent: { freigaben: [], count: 0, saturated: false } };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

/** Each leaf's path, and whether it is a string or a sentence with values. */
function form(k: object, prefix = ""): string[] {
  return Object.entries(k).flatMap(([key, v]) =>
    typeof v === "object" && v !== null
      ? form(v, `${prefix}${key}.`)
      : [`${prefix}${key}:${typeof v}`],
  );
}

describe("the catalogues", () => {
  it("holds every German key in English, each of the same kind, and no other", () => {
    expect(form(en).sort()).toEqual(form(de).sort());
  });

  it("leaves no English string empty", () => {
    const leer = (k: object, prefix = ""): string[] =>
      Object.entries(k).flatMap(([key, v]) =>
        typeof v === "object" && v !== null
          ? leer(v, `${prefix}${key}.`)
          : typeof v === "string" && v.trim() === ""
            ? [`${prefix}${key}`]
            : [],
      );
    expect(leer(en)).toEqual([]);
  });
});

describe("Sprache", () => {
  it("re-renders a view in English from the account menu and keeps the choice", async () => {
    services();
    await open("/fristen?id=p-1");
    expect(
      await screen.findByRole("heading", { name: "Fristen überschritten", level: 1 }),
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /Konto, Erscheinungsbild und Sprache/ }));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "English" }));

    expect(await screen.findByRole("heading", { name: "Fristen overdue", level: 1 })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Today" })).toBeTruthy();
    expect(document.documentElement.lang).toBe("en");
    expect(localStorage.getItem("mako-desk.sprache")).toBe("en");
    // The Berlin instant, in the English style: 09:00 UTC is 11:00 in Berlin (CEST).
    const angaben = screen.getByRole("complementary", { name: "Details" });
    expect(within(angaben).getByText("02/10/2026, 11:00")).toBeTruthy();
    // The market's vocabulary stays German.
    expect(within(angaben).getByText("Marktpartner-ID")).toBeTruthy();
    expect(screen.getByText("History")).toBeTruthy();

    act(() => setSprache("de"));
    expect(
      await screen.findByRole("heading", { name: "Fristen überschritten", level: 1 }),
    ).toBeTruthy();
    expect(document.documentElement.lang).toBe("de");
    expect(localStorage.getItem("mako-desk.sprache")).toBeNull();
  });
});

describe("the list–detail–inspector layout", () => {
  it("moves the selection with the arrows, opens with ↵ and returns with esc", async () => {
    services();
    const router = await open("/fristen");
    const grid = await screen.findByRole("grid", { name: "Überschrittene Fristen" });
    expect(screen.getByText("Nichts ausgewählt")).toBeTruthy();

    const [first, second] = within(grid).getAllByRole("row");
    if (!first || !second) throw new Error("two rows expected");
    act(() => first.focus());
    // Focus moves the selection with it, and the address names the process.
    fireEvent.keyDown(document.activeElement ?? first, { key: "ArrowDown" });
    await waitFor(() => expect(second.getAttribute("aria-selected")).toBe("true"));
    await waitFor(() => expect(router.state.location.search).toEqual({ id: "p-2" }));
    expect(await screen.findByRole("heading", { name: "41203768152", level: 2 })).toBeTruthy();

    fireEvent.keyDown(second, { key: "Enter" });
    fireEvent.keyUp(second, { key: "Enter" });
    const detail = screen.getByRole("region", { name: "Überschrittene Fristen" });
    await waitFor(() => expect(document.activeElement).toBe(detail));

    fireEvent.keyDown(detail, { key: "Escape" });
    await waitFor(() => expect(document.activeElement).toBe(second));
  });

  it("opens a selection from the address", async () => {
    services();
    await open("/fristen?id=p-1");
    const row = await screen.findByRole("row", { name: /51238696012/ });
    expect(row.getAttribute("aria-selected")).toBe("true");
    expect(await screen.findByRole("heading", { name: "51238696012", level: 2 })).toBeTruthy();
  });
});

/** Every string of `k` marked ⟪…⟫, so a string that bypasses the catalogue shows bare. */
function pseudo<T>(k: T): T {
  if (typeof k === "string") return `⟪${k}⟫` as T;
  if (typeof k === "function")
    return ((...a: unknown[]) => `⟪${(k as (...a: unknown[]) => string)(...a)}⟫`) as T;
  return Object.fromEntries(Object.entries(k as object).map(([key, v]) => [key, pseudo(v)])) as T;
}

/** What is left of `text` once catalogue strings, the services' data and digits are taken out. */
function bare(text: string, data: string[]): string {
  let rest = text;
  // Innermost first: a sentence may hold a catalogue word as its value.
  for (let before = ""; before !== rest; ) {
    before = rest;
    rest = rest.replace(/⟪[^⟪⟫]*⟫/g, "");
  }
  for (const d of data) rest = rest.split(d).join("");
  // Formatted instants and counts, punctuation, key caps, and the verbatim marks.
  return rest
    .replace(/⌘[A-Z]\b/g, "")
    .replace(/\b(PID|UTC|mako|Desk|MaLo)\b/g, "")
    .replace(/[\d\s.,:;·—–/()+\-→←↑↓↵⌘×]/g, "");
}

/** Every text and accessible name inside `root` that did not come from the catalogue. */
function bypassing(root: Element, data: string[]): string[] {
  const out: string[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const text = n.textContent ?? "";
    if (bare(text, data)) out.push(text);
  }
  for (const el of root.querySelectorAll("[aria-label],[title],[placeholder]")) {
    for (const attr of ["aria-label", "title", "placeholder"]) {
      const v = el.getAttribute(attr);
      if (v && bare(v, data)) out.push(`${attr}="${v}"`);
    }
  }
  return out;
}

const DATA = [
  ...OVERDUE.flatMap((p) => [
    p.process_id,
    p.malo_id,
    p.family,
    p.partner_mp_id ?? "",
    p.deadline_source ?? "",
  ]),
  ...DEAD.flatMap((d) => [d.reason_detail, d.reason_label]),
  "UTILMD-1",
  // The dev identity at the sidebar's foot.
  "dev-admin",
  "DA",
].filter(Boolean);

/** The surfaces that speak only through the catalogue: the sidebar and the converted views. */
function converted(): Element[] {
  const main = document.querySelector("main");
  const view = main?.querySelector(":scope > :not(header)");
  const sidebar = document.querySelector("aside");
  if (!view || !sidebar) throw new Error("shell not rendered");
  return [view, sidebar];
}

describe("the catalogue is the only source of words", () => {
  for (const [path, ready] of [
    ["/fristen?id=p-1", "51238696012"],
    ["/dead-letters", "PID 99999 is not registered"],
  ] as const) {
    it(`leaves no bare string on ${path}`, async () => {
      katalogErsetzen(pseudo<Katalog>(de));
      services();
      await open(path);
      await screen.findAllByText(ready);
      // The Verlauf of a selected process has arrived too.
      if (path.startsWith("/fristen")) await screen.findByText(/UTILMD-1/);
      expect(converted().flatMap((r) => bypassing(r, DATA))).toEqual([]);
    });
  }
});

describe("the Assistent view speaks only through the catalogue", () => {
  it("leaves no bare string on /assistent", async () => {
    katalogErsetzen(pseudo<Katalog>(de));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) =>
        String(url).endsWith("/agents/catalog")
          ? Response.json({
              faehigkeiten: [
                {
                  name: "desk-assistant",
                  titel: "Desk-Assistent",
                  zweck: "Beantwortet Fragen zum Anker.",
                  version: "1.2.0",
                  digest: null,
                  aktiv: true,
                  ausloeser: ["de.mako.process.started"],
                  modell: "anthropic/claude-sonnet-5",
                  liest: [{ werkzeug: "processd/get_process", wozu: "Prozess lesen" }],
                  nur_lesend: true,
                  mit_freigabe: 2,
                  bilanz: null,
                },
              ],
            })
          : new Response("{}", { status: 404 }),
      ),
    );
    await open("/assistent");
    await screen.findByText("Desk-Assistent");
    const data = [
      "Desk-Assistent",
      "Beantwortet Fragen zum Anker.",
      "desk-assistant",
      "claude-sonnet-5",
      "anthropic",
      "Prozess lesen",
      "Prozessüberwachung",
      "processd",
      "agentd",
      "Assistent",
      ...DATA,
    ];
    expect(converted().flatMap((r) => bypassing(r, data))).toEqual([]);
  });
});

describe("the search and decision views speak only through the catalogue", () => {
  for (const [path, ready] of [
    ["/prozess", "⟪Welcher Prozess?⟫"],
    ["/kunden", "⟪Wen suchen Sie?⟫"],
    ["/freigaben", "⟪Nichts zu entscheiden.⟫"],
    ["/heute", "⟪Heute⟫"],
  ] as const) {
    it(`leaves no bare string on ${path}`, async () => {
      katalogErsetzen(pseudo<Katalog>(de));
      services();
      await open(path);
      await screen.findAllByText(ready);
      // A service's name is the deployment's word for it, shown as named.
      const data = [...Object.values(DIENST), ...DATA];
      expect(converted().flatMap((r) => bypassing(r, data))).toEqual([]);
    });
  }
});

describe("the Einrichtung view speaks only through the catalogue", () => {
  it("leaves no bare string on /einrichtung", async () => {
    const liste = JSON.parse(checklisteJson) as {
      dienste: { dienst: string; version?: string }[];
      punkte: { befunde: { text: string; pfad: string; quelle?: string }[] }[];
      berichte: { grund?: string; roh?: string }[];
    };
    const vorschlag = JSON.parse(vorschlagJson) as {
      dateien: { datei: string; dienst: string; zeilen: { text: string; quelle?: string }[] }[];
      vorbelegt: {
        rollen?: { mp_id?: string | null }[];
        dienste?: { name: string; url: string }[];
      };
    };
    katalogErsetzen(pseudo<Katalog>(de));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        const result =
          m.method === "tools/call" && m.params.name === "einrichtung_checkliste"
            ? { structuredContent: liste }
            : m.method === "tools/call" && m.params.name === "einrichtung_vorschlag"
              ? { structuredContent: vorschlag }
              : m.method === "tools/call"
                ? { structuredContent: { freigaben: [], count: 0, saturated: false } }
                : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/einrichtung");
    await screen.findByRole("list", { name: "⟪Punkte⟫" });
    // makod's findings, paths, sources and the answer it kept verbatim are data.
    const data = [
      ...liste.punkte.flatMap((p) => p.befunde.flatMap((b) => [b.text, b.pfad, b.quelle ?? ""])),
      ...liste.berichte.flatMap((b) => [b.grund ?? "", b.roh ?? ""]),
      // makod's proposal — its files, lines and sources, and the answers it
      // pre-fills — before the service names, so a file name is taken whole.
      ...vorschlag.dateien.flatMap((d) => [
        d.datei,
        d.dienst,
        ...d.zeilen.flatMap((z) => [z.text, z.quelle ?? "", `# ${z.quelle ?? ""}`]),
      ]),
      ...(vorschlag.vorbelegt.rollen ?? []).map((r) => r.mp_id ?? ""),
      ...(vorschlag.vorbelegt.dienste ?? []).flatMap((d) => [d.name, d.url]),
      ...liste.dienste.flatMap((d) => [d.dienst, d.version ?? ""]),
      ...Object.values(DIENST),
      // The Probelauf's example facts, the profile's values as JSON.
      ...PROBEN.map((p) => zeigen(p.beispiel)),
      ...DATA,
    ].filter(Boolean);
    expect(converted().flatMap((r) => bypassing(r, data))).toEqual([]);
  });
});
