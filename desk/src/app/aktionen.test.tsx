import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { vergessen, verweigern } from "../data/verweigert";
import { RolleProvider } from "./rollen";
import { createDeskRouter } from "./router";

// ⌘K acts on what it finds through the action registry, and the views decide
// through the same registry — so the request a decision sends is one request,
// whichever way the person took. Each test renders its own app.

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
  vergessen();
  localStorage.removeItem("mako-desk.rolle");
});

type Post = { url: string; body: unknown };

const FREIGABE = {
  id: "q-1",
  process_id: "p-1",
  pid: 55001,
  malo_id: "51238696012",
  marktrolle: "NB",
  grund: "Anmeldung eines verbundenen Lieferanten (Antwortfrist 2031-01-02)",
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
  expires_at: "2031-01-02T09:00:00Z",
  created_at: "2031-01-01T08:00:00Z",
  decided_by: null,
  decided_at: null,
};

const DISPUT = {
  id: "rc-1",
  process_id: "p-inv",
  pid: 31002,
  sender_mp_id: "9900357000004",
  rechnungsnummer: "NB-2031-0815",
  received_at: "2031-01-01T08:00:00Z",
  bo4e_version: "v202607",
  frist: "2031-01-20T23:00:00Z",
  abgesendet: true,
  befunde: [{ art: "TariffDeviation", bezeichnung: "Preis weicht ab", text: "Position 1." }],
};

const POSTEN = {
  id: "b-rev",
  malo_id: "51238696012",
  lf_mp_id: "9900357000004",
  rechnungsnummer: "R-b-rev",
  category: "STROM",
  period_from: "2031-01-01",
  period_to: "2031-01-31",
  total_brutto_eur: "119.00",
  risk_band: "REVIEW",
  risk_score: 60,
  risk_findings: [{ code: "TAX_GAP", weight: 40, message: "Befund TAX_GAP" }],
  created_at: "2031-02-01T08:00:00Z",
};

const GEPARKT = {
  key: "rechnung:R-1",
  art: "RECHNUNG",
  malo_id: "51238696012",
  lf_mp_id: "9900357000004",
  betrag_ct: 900,
  buchungsdatum: "2031-01-31",
  beleg: "R-1",
  code: 301,
  meldung: "period sealed through 2031-01-31",
  hinweis: "Die Periode ist festgeschrieben.",
  vorschlag_datum: "2031-02-01",
  seit: "2031-02-02T08:00:00Z",
};

const BILANZ = {
  tage: 30,
  laeufe: 17,
  offen: 1,
  abgeschlossen: { succeeded: 16 },
  hilfreich: { ja: 6, nein: 1 },
  vorschlag: { sent: 14, edited: 2, dismissed: 1 },
};

const KATALOG = [
  {
    name: "desk-assistant",
    titel: "Assistent im Desk",
    zweck: "Beantwortet Fragen.",
    version: "1.3.0",
    digest: "d",
    aktiv: true,
    ausloeser: [],
    modell: "anthropic/claude-sonnet-5",
    liest: [],
    nur_lesend: true,
    mit_freigabe: 0,
    bilanz: BILANZ,
  },
];

/** The proposal waiting on q-1, as `/vorschlaege` answers it. */
const ABLEHNEN_A02 = {
  run: "r-7",
  status: "succeeded",
  disclosure: "AI-generated.",
  saetze: [],
  vorschlag: {
    aktion: "ablehnen",
    antwortcode: "A02",
    begruendung: "Die MaLo nimmt nicht an der Marktkommunikation teil.",
    quelle: "processd/get_queue_entry",
    belegt: true,
  },
  schritte: [],
  hilfreich: null,
};

/** Every read the four decision sources make, and every write recorded. */
function deskFetch(posts: Post[], opts: { wartend?: Record<string, unknown> } = {}) {
  return vi.fn(async (url: string, init?: RequestInit) => {
    if (
      url.includes("/api/v1/queue/") ||
      url.startsWith("/svc/invoicd/api/") ||
      url.startsWith("/svc/billingd/api/") ||
      url.startsWith("/svc/accountingd/api/") ||
      url.endsWith("/bewertung")
    ) {
      posts.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null });
      return new Response(null, { status: url.endsWith("/bewertung") ? 204 : 200 });
    }
    if (url === "/svc/agentd/api/v1/agents/catalog")
      return Response.json({ faehigkeiten: KATALOG });
    if (url.startsWith("/svc/agentd/api/v1/vorschlaege"))
      return Response.json({ freigabe: { "q-1": opts.wartend ?? null } });
    if (url.startsWith("/svc/agentd/")) return Response.json([]);
    const m = JSON.parse(String(init?.body));
    const name = m.method === "tools/call" ? m.params.name : null;
    const out: Record<string, unknown> = {
      list_pending_approvals: { freigaben: [FREIGABE], count: 1, saturated: false },
      list_disputes: { disputes: [DISPUT], count: 1, saturated: false },
      list_review_queue: { records: [POSTEN], count: 1, saturated: false },
      list_parked_bookings: { buchungen: [GEPARKT], count: 1, saturated: false },
      list_open_processes: { prozesse: [], weiter: null },
      list_overdue_processes: { overdue: [], count: 0, saturated: false, limit: 500 },
      list_anmeldungen: { anmeldungen: [], gesamt: 0 },
      list_korrekturbedarf: { korrekturbedarf: [] },
      kunden_zu_malos: { kunden: [] },
      partner_namen: { partner: [] },
    };
    // Anything else is a process's Verlauf, empty.
    const result = name
      ? { structuredContent: out[name] ?? { process_id: "p-1", workflow_name: "w", eintraege: [] } }
      : {};
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
      headers: { "content-type": "application/json", "mcp-session-id": "s" },
    });
  });
}

/** ⌘K, then the entry the identifier finds. */
async function findenUndWaehlen(kennung: string, art: RegExp) {
  fireEvent.keyDown(window, { key: "k", metaKey: true });
  const suche = await screen.findByRole("searchbox", { name: "Befehl oder Geschäftsschlüssel" });
  fireEvent.change(suche, { target: { value: kennung } });
  fireEvent.click(await screen.findByRole("menuitem", { name: art }));
  return screen.findByRole("listbox", { name: "Aktionen" });
}

/** Choose an action by its verb in the panel. */
function aktion(verb: string) {
  fireEvent.click(screen.getByRole("option", { name: new RegExp(`^${verb}`) }));
}

/** Enter in the action's form: held, then sent at once by pagehide. */
function sendenUndAbgeben() {
  fireEvent.keyDown(screen.getByRole("group", { name: /Parameter/ }), { key: "Enter" });
  act(() => {
    window.dispatchEvent(new Event("pagehide"));
  });
}

const flush = () => act(() => window.dispatchEvent(new Event("pagehide")));

describe("the action registry", () => {
  it("sends a Freigabe's refusal from ⌘K exactly as its view does", async () => {
    const view: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(view));
    await open("/freigaben?id=q-1");
    await screen.findByRole("heading", { name: /verbundenen Lieferanten/ });
    fireEvent.click(screen.getByRole("radio", { name: "Ablehnen" }));
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.change(screen.getByRole("textbox", { name: /Bemerkung/ }), {
      target: { value: "Nicht im Netz" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen" }));
    flush();
    await waitFor(() => expect(view).toHaveLength(1));
    cleanup();

    const palette: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(palette));
    await open("/heute");
    await findenUndWaehlen("q-1", /Freigabe/);
    aktion("Ablehnen");
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.change(screen.getByRole("textbox", { name: /Bemerkung/ }), {
      target: { value: "Nicht im Netz" },
    });
    sendenUndAbgeben();
    await waitFor(() => expect(palette).toHaveLength(1));
    expect(palette).toEqual(view);
    expect(palette[0]).toEqual({
      url: "/svc/processd/api/v1/queue/q-1/reject",
      body: { antwort_code: "A02", antwort_tree: "E_0622", bemerkung: "Nicht im Netz" },
    });
  });

  it("sends a Disput's Beilegen from ⌘K exactly as its view does", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const view: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(view));
    await open("/heute");
    fireEvent.click(await screen.findByRole("button", { name: /Rechnung NB-2031-0815/ }));
    fireEvent.click(screen.getByRole("radio", { name: "Beilegen" }));
    fireEvent.change(screen.getByRole("textbox", { name: /Wie wurde er geklärt/ }), {
      target: { value: "Gutschrift kommt" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Beilegen" }));
    flush();
    await waitFor(() => expect(view).toHaveLength(1));
    cleanup();

    const palette: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(palette));
    await open("/heute");
    await findenUndWaehlen("rc-1", /Rechnungsdisput/);
    aktion("Beilegen");
    fireEvent.change(screen.getByRole("textbox", { name: /Wie wurde er geklärt/ }), {
      target: { value: "Gutschrift kommt" },
    });
    sendenUndAbgeben();
    await waitFor(() => expect(palette).toHaveLength(1));
    expect(palette).toEqual(view);
  });

  it("sends a Prüfposten's Geprüft from ⌘K exactly as its view does", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const view: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(view));
    await open("/heute");
    fireEvent.click(await screen.findByRole("button", { name: /Befund TAX_GAP/ }));
    fireEvent.click(screen.getByRole("button", { name: "Geprüft" }));
    flush();
    await waitFor(() => expect(view).toHaveLength(1));
    cleanup();

    const palette: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(palette));
    await open("/heute");
    const aktionen = await findenUndWaehlen("b-rev", /Rechnungsprüfung/);
    // A Prüfposten has one action, and nothing to prefill.
    expect(within(aktionen).getAllByRole("option")).toHaveLength(1);
    aktion("Geprüft");
    expect(screen.queryByRole("region", { name: "Von einem Modell verfasst" })).toBeNull();
    sendenUndAbgeben();
    await waitFor(() => expect(palette).toHaveLength(1));
    expect(palette).toEqual(view);
  });

  it("sends a geparkte Buchung's Verwerfen from ⌘K exactly as its view does", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const view: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(view));
    await open("/heute");
    fireEvent.click(
      await screen.findByRole("button", { name: /Rechnung 9,00\s€ — Periode festgeschrieben/ }),
    );
    fireEvent.click(screen.getByRole("radio", { name: "Verwerfen" }));
    fireEvent.change(screen.getByRole("textbox", { name: /Warum wird sie verworfen/ }), {
      target: { value: "Doppelt erfasst" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Buchung verwerfen" }));
    flush();
    await waitFor(() => expect(view).toHaveLength(1));
    cleanup();

    const palette: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(palette));
    await open("/heute");
    await findenUndWaehlen("rechnung:R-1", /Geparkte Buchung/);
    // ⌘Enter takes the secondary action straight from the panel.
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Aktion filtern" }), {
      key: "Enter",
      metaKey: true,
    });
    fireEvent.change(screen.getByRole("textbox", { name: /Warum wird sie verworfen/ }), {
      target: { value: "Doppelt erfasst" },
    });
    sendenUndAbgeben();
    await waitFor(() => expect(palette).toHaveLength(1));
    expect(palette).toEqual(view);
  });
});

describe("the action panel", () => {
  it("lists exactly a Freigabe's two actions, each with its key, and filters them by typing", async () => {
    vi.stubGlobal("fetch", deskFetch([]));
    await open("/heute");
    const aktionen = await findenUndWaehlen("51238696012", /Freigabe/);
    const optionen = within(aktionen).getAllByRole("option");
    expect(optionen.map((o) => o.textContent)).toEqual(["Freigeben↵", "Ablehnen⌘↵"]);
    fireEvent.change(screen.getByRole("combobox", { name: "Aktion filtern" }), {
      target: { value: "abl" },
    });
    expect(
      within(aktionen)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["Ablehnen⌘↵"]);
  });

  it("offers as the answer code only the codes its tree publishes, with their wording", async () => {
    vi.stubGlobal("fetch", deskFetch([]));
    await open("/heute");
    await findenUndWaehlen("q-1", /Freigabe/);
    aktion("Ablehnen");
    const codes = screen.getByRole("radiogroup", { name: /Antwortcode aus E_0622/ });
    expect(
      within(codes)
        .getAllByRole("radio")
        .map((r) => r.closest("label")?.textContent),
    ).toEqual(["A01Marktlokation nicht gefunden", "A02nimmt nicht an der Marktkommunikation teil"]);
    // Without a chosen code nothing can be sent.
    expect(screen.getByRole("button", { name: /^Ablehnen/ }).hasAttribute("disabled")).toBe(true);
  });

  it("shows an action its service refused this session in the service's words, and sends nothing", async () => {
    const posts: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(posts));
    verweigern({ dienst: "processd", aktion: "entscheiden" }, "Reject denied for this principal");
    await open("/heute");
    const aktionen = await findenUndWaehlen("q-1", /Freigabe/);
    expect(within(aktionen).getAllByText("Reject denied for this principal")).toHaveLength(2);
    expect(
      within(aktionen)
        .getAllByRole("option")
        .every((o) => o.getAttribute("aria-disabled") === "true"),
    ).toBe(true);
    const filter = screen.getByRole("combobox", { name: "Aktion filtern" });
    fireEvent.keyDown(filter, { key: "Enter" });
    fireEvent.keyDown(filter, { key: "Enter", metaKey: true });
    expect(screen.queryByRole("group", { name: /Parameter/ })).toBeNull();
    flush();
    expect(posts).toEqual([]);
  });

  it("opens on the actions of the focused Heute row, and Esc goes back to the search", async () => {
    vi.stubGlobal("fetch", deskFetch([]));
    await open("/heute");
    const zeile = await screen.findByRole("button", { name: /verbundenen Lieferanten/ });
    act(() => zeile.focus());
    fireEvent.keyDown(zeile, { key: "k", metaKey: true });
    const aktionen = await screen.findByRole("listbox", { name: "Aktionen" });
    expect(within(aktionen).getAllByRole("option")).toHaveLength(2);
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Aktion filtern" }), {
      key: "Escape",
    });
    expect(
      await screen.findByRole("searchbox", { name: "Befehl oder Geschäftsschlüssel" }),
    ).toBeTruthy();
    expect(screen.queryByRole("listbox", { name: "Aktionen" })).toBeNull();
  });

  it("leaves every letter typed into its search to the search, over a view that answers letters", async () => {
    vi.stubGlobal("fetch", deskFetch([]));
    await open("/freigaben?id=q-1");
    await screen.findByRole("heading", { name: /verbundenen Lieferanten/ });
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    await screen.findByRole("listbox", { name: "Aktionen" });
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Aktion filtern" }), {
      key: "Escape",
    });
    const suche = await screen.findByRole("searchbox", { name: "Befehl oder Geschäftsschlüssel" });
    act(() => suche.focus());
    // The search's list hears each key too, re-dispatched with the focus left
    // in the field: J/K step and A/R choose there, so they must not take it.
    for (const key of ["r", "k", "a", "j", "u", "?"]) {
      expect(fireEvent.keyDown(suche, { key })).toBe(true);
    }
    // Behind the palette, the decision was not chosen by the letters either.
    const ablehnen = screen.getByRole("radio", { name: "Ablehnen", hidden: true });
    expect((ablehnen as HTMLInputElement).checked).toBe(false);
  });
});

describe("the prefill", () => {
  it("marks the code the waiting proposal filled, and sends it unchanged", async () => {
    const posts: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(posts, { wartend: ABLEHNEN_A02 }));
    await open("/heute");
    const aktionen = await findenUndWaehlen("q-1", /Freigabe/);
    // The proposed action is marked in the panel.
    await within(aktionen).findByText("Vorschlag");
    aktion("Ablehnen");
    await waitFor(() =>
      expect((screen.getByRole("radio", { name: /A02/ }) as HTMLInputElement).checked).toBe(true),
    );
    const marke = screen.getByRole("region", { name: "Von einem Modell verfasst" });
    expect(marke.textContent).toMatch(/^AgentAI-generated\./);
    // The skill's record, as M74 shows it under every proposal.
    expect(await within(marke).findByText(/In 30 Tagen:/)).toBeTruthy();
    expect(
      screen.getByRole("radiogroup", { name: /Antwortcode/ }).getAttribute("data-vorschlag"),
    ).toBe("true");
    sendenUndAbgeben();
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A02", antwort_tree: "E_0622" },
        },
        { url: "/svc/agentd/api/v1/runs/r-7/bewertung", body: { vorschlag: "sent" } },
      ]),
    );
  });

  it("drops the mark once the person changes the code, and records the proposal as edited", async () => {
    const posts: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(posts, { wartend: ABLEHNEN_A02 }));
    await open("/heute");
    const aktionen = await findenUndWaehlen("q-1", /Freigabe/);
    await within(aktionen).findByText("Vorschlag");
    aktion("Ablehnen");
    await screen.findByRole("region", { name: "Von einem Modell verfasst" });
    fireEvent.click(screen.getByRole("radio", { name: /A01/ }));
    expect(screen.queryByRole("region", { name: "Von einem Modell verfasst" })).toBeNull();
    expect(
      screen.getByRole("radiogroup", { name: /Antwortcode/ }).getAttribute("data-vorschlag"),
    ).toBe("false");
    sendenUndAbgeben();
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A01", antwort_tree: "E_0622" },
        },
        { url: "/svc/agentd/api/v1/runs/r-7/bewertung", body: { vorschlag: "edited" } },
      ]),
    );
  });

  it("closes onto the entry with the hold's ring, and ⌘Z within the hold sends nothing", async () => {
    const posts: Post[] = [];
    vi.stubGlobal("fetch", deskFetch(posts, { wartend: ABLEHNEN_A02 }));
    const router = await open("/heute");
    const aktionen = await findenUndWaehlen("q-1", /Freigabe/);
    await within(aktionen).findByText("Vorschlag");
    aktion("Ablehnen");
    await waitFor(() =>
      expect((screen.getByRole("radio", { name: /A02/ }) as HTMLInputElement).checked).toBe(true),
    );
    // ↵ where the focus is — on the chosen code, whose radio group must not swallow it.
    const code = screen.getByRole("radio", { name: /A02/ });
    expect(document.activeElement).toBe(code);
    fireEvent.keyDown(code, { key: "Enter" });
    // The palette is gone; the entry carries the countdown.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(router.state.location.pathname).toBe("/freigaben"));
    expect(await screen.findAllByRole("timer", { name: /Gleich gesendet/ })).not.toHaveLength(0);
    fireEvent.keyDown(document.body, { key: "z", metaKey: true });
    await waitFor(() => expect(screen.queryAllByRole("timer")).toHaveLength(0));
    flush();
    expect(posts).toEqual([]);
  });
});
