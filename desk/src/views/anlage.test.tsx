import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// The Anlage page: einsd's facts with the Förderende large, the settled
// months as a strip — paid where accountingd's EEG payouts say so — each
// month opening einsd's statement with the legal basis of every position.

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
});

const ANTWORT: Record<string, unknown> = {
  "/svc/einsd/api/v1/anlagen/TR-1": {
    tr_id: "TR-1",
    malo_id: "51238696012",
    erzeugungsart: "SOLAR",
    leistung_kwp: "9.8",
    eeg_gesetz: 2023,
    inbetriebnahme: "2023-05-01",
    foerderendedatum: "2043-12-31",
    mastr_registriert: true,
    mastr_nummer: "SEE912345678901",
    verguetungsform: "Einspeiseverguetung",
    verguetungssatz_ct: "8.11",
    direktvermarktung: false,
    status: "aktiv",
  },
  "/svc/einsd/api/v1/anlagen/TR-1/settlements": {
    tr_id: "TR-1",
    count: 2,
    saturated: false,
    settlements: [
      {
        id: "s-2",
        tr_id: "TR-1",
        billing_year: 2026,
        billing_month: 10,
        status: "settled",
        settlement_eur: "61.20",
        settled_at: "2026-11-03T08:00:00Z",
        settlement_model: "eeg",
        is_correction: false,
      },
      {
        id: "s-1",
        tr_id: "TR-1",
        billing_year: 2026,
        billing_month: 9,
        status: "settled",
        settlement_eur: "88.40",
        settled_at: "2026-10-03T08:00:00Z",
        settlement_model: "eeg",
        is_correction: false,
      },
    ],
  },
  "/svc/accountingd/api/v1/eeg/payouts": {
    count: 1,
    saturated: false,
    payouts: [
      {
        payout_id: "p-1",
        tr_id: "TR-1",
        malo_id: "51238696012",
        billing_year: 2026,
        billing_month: 9,
        amount_ct: 8840,
        amount_eur: "88.40",
        settled_at: "2026-10-10T08:00:00Z",
        created_at: "2026-10-03T08:00:00Z",
        creditor_name: "A",
        end_to_end_ref: "e",
        payment_type: "eeg",
      },
    ],
  },
  "/svc/einsd/api/v1/anlagen/TR-1/settlements/2026/10/erklaerung": {
    tr_id: "TR-1",
    billing_year: 2026,
    billing_month: 10,
    status: "settled",
    settled_at: "2026-11-03T08:00:00Z",
    settlement_model: "eeg",
    interpretation: "Einspeisevergütung nach anzulegendem Wert.",
    verlaengerungsanspruch_qh: 0,
    positions: [
      {
        description: "Einspeisevergütung",
        kwh: "754.6",
        rate_ct_kwh: "8.11",
        eur: "61.20",
        legal_basis: "§ 21 EEG 2023",
      },
    ],
  },
};

describe("the Anlage page", () => {
  it("shows the facts, the months settled and paid, and a month's statement with its basis", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const pfad = url.split("?")[0] ?? "";
        if (pfad in ANTWORT) return Response.json(ANTWORT[pfad]);
        if (url.startsWith("/svc/agentd/")) return Response.json([]);
        if (!init?.body) return Response.json({});
        const m = JSON.parse(String(init.body));
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: {} }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/anlage/TR-1");
    expect(
      await screen.findByRole("heading", { level: 1, name: /Photovoltaik · 9,8 kWp/ }),
    ).toBeTruthy();
    expect(screen.getByRole("group", { name: "Förderende" }).textContent).toContain("31.12.2043");
    const streifen = await screen.findByRole("list", {
      name: "Abrechnungen der letzten zwölf Monate",
    });
    const monate = within(streifen).getAllByRole("listitem");
    expect(monate.map((m) => [m.textContent?.slice(0, 7), m.getAttribute("data-bezahlt")])).toEqual(
      [
        ["09/2026", "true"],
        ["10/2026", "false"],
      ],
    );
    fireEvent.click(within(monate[1] as HTMLElement).getByRole("button"));
    const positionen = await screen.findByRole("list", { name: "Positionen der Abrechnung" });
    expect(within(positionen).getByRole("button", { name: /§ 21 EEG 2023/ })).toBeTruthy();
  });
});
