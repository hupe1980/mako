import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// A Gegenpartei's record, as processd keeps it: how its answers arrived
// against their Fristen, what it refused and with which code, every process
// behind a figure — a share only from processd's own threshold, and the
// whole page exportable as evidence.

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

const akte = (quote: number | null) => ({
  mp_id: "9800357000005",
  rolle: "NB",
  von: "2026-08-01",
  bis: "2026-11-17",
  erstellt_um: "2026-11-17T09:00:00Z",
  gezaehlt: 13,
  fristgerecht: 11,
  verspaetet: 1,
  unbeantwortet: 1,
  offen: 0,
  abgebrochen: 0,
  verspaetung: { median_minuten: 180, max_minuten: 180 },
  quote_fristgerecht: quote,
  ablehnungen: [
    { code: "A16", codeliste: "E_3005", bedeutung: "Marktlokation nicht im Netzgebiet", anzahl: 1 },
  ],
  ausserhalb: { ohne_frist: 0, schuldner_unbestimmt: 0 },
  prozesse: [
    {
      process_id: "p-1",
      pid: 44001,
      kategorie: "verspaetet",
      gestartet_um: "2026-10-01T08:00:00Z",
      frist: "2026-10-05T22:00:00Z",
      beantwortet_um: "2026-10-06T01:00:00Z",
      verspaetung_minuten: 180,
      antwort_code: "A16",
      antwort_codeliste: "E_3005",
      abgelehnt: true,
    },
  ],
  definitionen: [{ begriff: "fristgerecht", definition: "beantwortet vor Ablauf der Frist" }],
});

function stub(quote: number | null) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/call" && m.params.name === "get_counterparty_record")
        result = { structuredContent: akte(quote) };
      else if (m.method === "tools/call" && m.params.name === "partner_namen")
        result = {
          structuredContent: {
            partner: [{ mp_id: "9800357000005", name: "Gasnetz Nachbarstadt" }],
          },
        };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("the Gegenpartei page", () => {
  it("names the partner, counts below the threshold as counts, and lists every refusal with its wording", async () => {
    stub(null);
    await open("/gegenpartei/9800357000005/NB");
    expect(
      await screen.findByRole("heading", { level: 1, name: /Gasnetz Nachbarstadt/ }),
    ).toBeTruthy();
    const zahlen = screen.getByRole("list", { name: "Kennzahlen" });
    expect(zahlen.textContent).toContain("11 von 13");
    expect(zahlen.textContent).not.toContain("%");
    const ablehnungen = screen.getByRole("list", { name: "Ablehnungen" });
    expect(within(ablehnungen).getByText(/Marktlokation nicht im Netzgebiet/)).toBeTruthy();
    expect(screen.getByRole("grid", { name: "Prozesse dieser Gegenpartei" })).toBeTruthy();
    const export_ = screen.getByRole("link", { name: "Als Nachweis exportieren" });
    expect(export_.getAttribute("href")).toBe(
      "/svc/processd/api/v1/counterparties/9800357000005/NB?von=2026-08-01&bis=2026-11-17&format=csv",
    );
    fireEvent.click(screen.getByRole("button", { name: "Begriffe" }));
    expect(await screen.findByText("beantwortet vor Ablauf der Frist")).toBeTruthy();
  });

  it("shows processd's share where processd states one", async () => {
    stub(91.7);
    await open("/gegenpartei/9800357000005/NB");
    const zahlen = await screen.findByRole("list", { name: "Kennzahlen" });
    expect(zahlen.textContent).toMatch(/92\s?%/);
  });
});
