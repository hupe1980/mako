import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// „Nachrechnen": billingd recomputes an invoice from its inputs; the Desk
// shows the verdict first, then every line with where its quantity and price
// came from, the rule and the arithmetic — none of it computed here.

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

const zeile = (position: number, stated: string, recomputed: string, matches: boolean) => ({
  position,
  text: position === 1 ? "Arbeitspreis" : "Grundpreis",
  quantity: position === 1 ? "3200" : "12",
  unit: position === 1 ? "kWh" : "Monat",
  unit_price_eur: position === 1 ? "0.3120" : "11.90",
  stated_eur: stated,
  recomputed_eur: recomputed,
  difference_eur: matches ? "0.00" : "1.20",
  matches,
  formula: position === 1 ? "3200 kWh × 0,3120 €/kWh" : "12 Monate × 11,90 €",
  rules: position === 1 ? ["§ 40 Abs. 2 Nr. 6 EnWG"] : [],
  price_source: "Produkt Ökostrom Klassik, Preisstand 01.06.2026",
  quantity_source: position === 1 ? "Ablesung 31.12.2026, Messwert" : null,
  estimated: false,
});

function billingd(verdict: Record<string, unknown>, lines: unknown[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      const result =
        m.method === "tools/call" && m.params.name === "recompute_invoice"
          ? {
              structuredContent: {
                record_id: "b-1",
                rechnungsnummer: "R-2026-0042",
                public_link: "https://portal.example/nachrechnen/tok-123",
                malo_id: "51238696012",
                period_from: "2026-01-01",
                period_to: "2026-12-31",
                vorauszahlungen_eur: "960.00",
                verdict: {
                  matches: true,
                  arithmetic_matches: true,
                  seal_holds: true,
                  mismatched_positions: [],
                  storniert: false,
                  ...verdict,
                },
                lines,
                totals: [
                  {
                    kind: "BRUTTO",
                    rate_percent: null,
                    stated_eur: "1071.42",
                    recomputed_eur: "1071.42",
                    matches: true,
                  },
                  {
                    kind: "ZU_ZAHLEN",
                    rate_percent: null,
                    stated_eur: "111.42",
                    recomputed_eur: "111.42",
                    matches: true,
                  },
                ],
              },
            }
          : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("Nachrechnen", () => {
  it("leads with „Stimmt überein“ and the recomputed total, then each line with its sources", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    billingd({}, [zeile(1, "998.40", "998.40", true), zeile(2, "142.80", "142.80", true)]);
    await open("/rechnung/b-1");
    const urteil = await screen.findByRole("region", { name: "Stimmt überein" });
    expect(urteil.textContent).toMatch(/1\.071,42\s?€/);
    const zeilen = screen.getByRole("list", { name: "Berechnet aus" });
    const erste = within(zeilen).getAllByRole("listitem")[0] as HTMLElement;
    expect(erste.textContent).toContain("Arbeitspreis");
    expect(erste.textContent).toContain("3200 kWh × 0,3120 €/kWh");
    expect(erste.textContent).toContain("Ablesung 31.12.2026, Messwert");
    expect(within(erste).getByRole("button", { name: /§ 40 Abs\. 2 Nr\. 6 EnWG/ })).toBeTruthy();
    // The customer's own page, to copy into a reply.
    expect(screen.getByText("https://portal.example/nachrechnen/tok-123")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Link kopieren" })).toBeTruthy();
  });

  it("says „Weicht ab“ and names the line that does not recompute", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    billingd({ matches: false, arithmetic_matches: false, mismatched_positions: [2] }, [
      zeile(1, "998.40", "998.40", true),
      zeile(2, "144.00", "142.80", false),
    ]);
    await open("/rechnung/b-1");
    const urteil = await screen.findByRole("region", { name: "Weicht ab" });
    expect(urteil.textContent).toContain("Position 2");
    const zweite = within(screen.getByRole("list", { name: "Berechnet aus" })).getAllByRole(
      "listitem",
    )[1] as HTMLElement;
    expect(zweite.getAttribute("data-stimmt")).toBe("false");
    expect(zweite.textContent).toMatch(/144,00\s?€/);
    expect(zweite.textContent).toMatch(/142,80\s?€/);
  });
});
