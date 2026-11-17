import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// The Lokation page reads the services' REST API: marktd's MaLo, Versorgung
// and Lokationsbündel, edmd's completeness and Lastgang, accountingd's Konto.
// Each panel answers or fails on its own.

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
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.clear();
});

const MALO = "51238696012";

const ANTWORTEN: Record<string, unknown> = {
  [`/svc/marktd/api/v1/malos/${MALO}`]: {
    malo_id: MALO,
    sparte: "STROM",
    netzebene: "NSP",
    bilanzierungsmethode: "SLP",
    energierichtung: "AUSSP",
    lokationsbuendel_objektcode: "9991000000044",
    rollenzuordnung: [],
    data: {},
    version: 3,
  },
  [`/svc/marktd/api/v1/versorgung/${MALO}/history`]: {
    items: [
      {
        id: 2,
        malo_id: MALO,
        lieferstatus: "Beliefert",
        nb_mp_id: "9900357000004",
        valid_from: "2026-06-30T22:00:00Z",
        version: 2,
        zuordnungen: [{ lf_mp_id: "9905550000010", prozent: "100", status: "Aktiv" }],
      },
      {
        id: 1,
        malo_id: MALO,
        lieferstatus: "Grundversorgung",
        nb_mp_id: "9900357000004",
        valid_from: "2025-12-31T23:00:00Z",
        version: 1,
        zuordnungen: [],
      },
    ],
    page: 0,
    size: 50,
    total: 2,
  },
  [`/svc/marktd/api/v1/malos/${MALO}/lokationsbuendel`]: {
    malo_id: MALO,
    stichtag: "2026-11-17",
    lokationsbuendelcodes: ["9991000000044"],
    objekte: [
      { id: MALO, typ: "MALO" },
      { id: "DE0001234567890000000000000000001", typ: "MELO" },
    ],
    zaehler: [{ melo_id: "DE0001234567890000000000000000001", zaehler_id: "1ESY1160001234" }],
    zuordnungen: [
      {
        von_id: MALO,
        von_typ: "MALO",
        nach_id: "DE0001234567890000000000000000001",
        nach_typ: "MELO",
      },
    ],
  },
  [`/svc/edmd/api/v1/coverage/${MALO}`]: {
    malo_id: MALO,
    sparte: "STROM",
    von: "2026-10-19",
    bis: "2026-10-21",
    gemessen: 1,
    ersatzwert: 1,
    korrigiert: 0,
    fehlt: 1,
    tage: [
      { tag: "2026-10-19", zustand: "gemessen" },
      { tag: "2026-10-20", zustand: "ersatzwert" },
      { tag: "2026-10-21", zustand: "fehlt" },
    ],
  },
  [`/svc/edmd/api/v1/timeseries/${MALO}`]: {
    malo_id: MALO,
    from: "2026-11-10T23:00:00Z",
    to: "2026-11-17T23:00:00Z",
    count: 0,
    readings: [],
  },
  "/svc/marktd/api/v1/lokationsbuendelstrukturen": {
    strukturen: [{ code: "9991000000044", bezeichnung: "MaLo mit einer MeLo, Standardlastprofil" }],
  },
  [`/svc/edmd/api/v1/quality/${MALO}`]: {
    malo_id: MALO,
    algorithm: "v1",
    grade: "B",
    coverage_pct: "96.7",
    gaps_detected: 1,
    has_warnings: true,
    interval_count: 2784,
    intervals_consistent: true,
    outlier_intervals: [],
    spike_intervals: [],
    window_from: "2026-10-19T22:00:00Z",
    window_to: "2026-11-17T23:00:00Z",
    zero_run_length: 0,
    expected_intervals: 2880,
  },
  [`/svc/edmd/api/v1/corrections/${MALO}`]: {
    malo_id: MALO,
    count: 1,
    saturated: false,
    from: null,
    to: null,
    corrections: [
      {
        correction_id: "c-1",
        dtm_from: "2026-10-20T10:00:00Z",
        dtm_to: "2026-10-20T10:15:00Z",
        obis_code: "1-1:1.29.0",
        original_kwh: "0.000",
        corrected_kwh: "0.214",
        original_quality: "FAULTY",
        corrected_quality: "CORRECTED",
        corrected_at: "2026-10-21T08:00:00Z",
        corrected_by: "msb",
        reason: "Zählerstand nachgemeldet",
        source: "MSCONS",
      },
    ],
  },
  [`/svc/accountingd/api/v1/accounts/${MALO}/balance`]: {
    malo_id: MALO,
    lf_mp_id: "9905550000010",
    balance_ct: 12_345,
    balance_eur: "123.45",
    abschlag_ct: 8_000,
    status: "overdue",
  },
  [`/svc/accountingd/api/v1/accounts/${MALO}/ledger`]: {
    malo_id: MALO,
    lf_mp_id: "9905550000010",
    opening_ct: 0,
    lines: [
      {
        entry_id: "e-1",
        booking_date: "2026-11-01",
        entry_type: "RECHNUNG",
        beleg: "R-2026-0042",
        side: "debit",
        amount_ct: 12_345,
        signed_ct: 12_345,
        running_ct: 12_345,
      },
    ],
  },
};

function stub() {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | Request, init?: RequestInit) => {
      const url =
        typeof input === "string"
          ? input
          : `${new URL(input.url).pathname}${new URL(input.url).search}`;
      urls.push(url);
      const pfad = url.split("?")[0] ?? "";
      if (pfad in ANTWORTEN) return Response.json(ANTWORTEN[pfad]);
      if (pfad.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: {} }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
  return urls;
}

describe("the Lokation page", () => {
  it("reads every panel over REST and draws each as the services state it", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const urls = stub();
    await open(`/lokation/${MALO}?knoten=DE0001234567890000000000000000001`);
    const h1 = await screen.findByRole("heading", { level: 1, name: /Marktlokation 51238696012/ });
    expect(await within(h1).findByText("Beliefert")).toBeTruthy();
    // Versorgung: oldest first, the latest open-ended.
    const bands = await screen.findByRole("list", { name: "Versorgung" });
    const zeilen = within(bands).getAllByRole("listitem");
    expect(zeilen.map((z) => z.getAttribute("data-status"))).toEqual([
      "Grundversorgung",
      "Beliefert",
    ]);
    expect(zeilen[1]?.textContent).toContain("seit 01.07.2026");
    // Messkonzept: the MaLo and its MeLo with its Zähler.
    const svg = await screen.findByRole("img", { name: "Messkonzept" });
    expect(svg.querySelectorAll("g[data-typ]")).toHaveLength(2);
    expect(svg.textContent).toContain("Zähler 1ESY1160001234");
    // The bündel's code with marktd's BDEW wording; the node the page was opened on, marked.
    expect(await screen.findByText(/9991000000044 MaLo mit einer MeLo/)).toBeTruthy();
    expect(svg.querySelector('[aria-current="location"]')?.getAttribute("data-typ")).toBe("MELO");
    // edmd's quality and its corrections beside the ribbon.
    expect(await screen.findByText(/Qualität gut · 96\.7 % abgedeckt · 1 Lücken/)).toBeTruthy();
    expect((await screen.findByRole("list", { name: "Korrekturen" })).textContent).toContain(
      "Zählerstand nachgemeldet",
    );
    // Messwerte: one cell per day, as edmd states it.
    const tage = await screen.findByRole("list", { name: "Messwerte je Tag" });
    expect([...tage.querySelectorAll("li")].map((li) => li.getAttribute("data-zustand"))).toEqual([
      "gemessen",
      "ersatzwert",
      "fehlt",
    ]);
    // Konto: accountingd's balance and statement.
    expect((await screen.findByRole("list", { name: /Kontoauszug/ })).textContent).toContain(
      "R-2026-0042",
    );
    // Every panel read its service's REST path.
    expect(urls.some((u) => u.startsWith(`/svc/edmd/api/v1/coverage/${MALO}?from=`))).toBe(true);
    for (const pfad of [
      `/svc/marktd/api/v1/malos/${MALO}`,
      `/svc/marktd/api/v1/versorgung/${MALO}/history`,
      `/svc/edmd/api/v1/timeseries/${MALO}?from=`,
      `/svc/accountingd/api/v1/accounts/${MALO}/ledger`,
    ])
      expect(
        urls.some((u) => u.startsWith(pfad)),
        pfad,
      ).toBe(true);
  });
});
