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

function billingd(
  verdict: Record<string, unknown>,
  lines: unknown[],
  delivery?: () => Response,
  reference = "R-2026-0042",
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/outputd/api/v1/documents/")) {
        const read = new URL(url, "http://localhost");
        expect(read.pathname).toBe("/svc/outputd/api/v1/documents/by-subject/INVOICE");
        expect(read.searchParams.get("subject_ref")).toBe(reference);
        return delivery?.() ?? Response.json({ code: "DOCUMENT_NOT_FOUND" }, { status: 404 });
      }
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      const result =
        m.method === "tools/call" && m.params.name === "recompute_invoice"
          ? {
              structuredContent: {
                record_id: "b-1",
                rechnungsnummer: reference,
                dispatched_at: "2026-11-03T11:00:00Z",
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
  it("preserves a supplier’s invoice separators through the gateway", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const reference = "R/2026 + #?&";
    billingd(
      {},
      [],
      () => Response.json({ kind: "INVOICE", subject_ref: reference, deliveries: [] }),
      reference,
    );
    await open("/rechnung/b-1");
    expect(await screen.findByText("Für die Bereitstellung liegt kein Nachweis vor.")).toBeTruthy();
  });
  it("keeps recorded issuance separate from failed and suppressed delivery tracks", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    billingd({}, [], () =>
      Response.json({
        subject_ref: "R-2026-0042",
        kind: "INVOICE",
        deliveries: [
          { channel: "EMAIL", status: "FAILED" },
          { channel: "POST", status: "SUPPRESSED" },
        ],
      }),
    );
    await open("/rechnung/b-1");
    expect(await screen.findByText(/Ausgestellt am 03.11.2026, 12:00/)).toBeTruthy();
    expect(await screen.findByText(/Für die Bereitstellung liegt kein Nachweis vor/)).toBeTruthy();
    expect(screen.getByText(/E-Mail · Zustellung fehlgeschlagen/)).toBeTruthy();
    expect(screen.getByText(/Post · Versand unterdrückt/)).toBeTruthy();
  });

  it("reports when the portal published a document even if its email only reached a relay", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    billingd({}, [], () =>
      Response.json({
        subject_ref: "R-2026-0042",
        kind: "INVOICE",
        deliveries: [
          { channel: "EMAIL", status: "SENT", first_sent_at: "2026-11-03T13:00:00Z" },
          {
            channel: "PORTAL",
            status: "DELIVERED",
            delivered_at: "2026-11-03T12:00:00Z",
            read_at: "2026-11-03T16:00:00Z",
          },
        ],
      }),
    );
    await open("/rechnung/b-1");
    expect(
      await screen.findByText(/Im Kundenportal bereitgestellt am 03.11.2026, 13:00/),
    ).toBeTruthy();
    expect(screen.getByText(/Im Kundenportal geöffnet am 03.11.2026, 17:00/)).toBeTruthy();
    expect(screen.getByText(/E-Mail · versendet · 03.11.2026, 14:00/)).toBeTruthy();
  });

  it.each([
    [
      () => Response.json({ code: "DOCUMENT_NOT_FOUND" }, { status: 404 }),
      "Für die Bereitstellung liegt kein Nachweis vor.",
    ],
    [() => Response.json({}, { status: 403 }), "Du hast keinen Zugriff auf die Zustellnachweise."],
    [() => Response.json({}, { status: 503 }), "Die Zustellnachweise sind derzeit nicht abrufbar."],
    [
      () => Response.json({ subject_ref: "another", kind: "INVOICE", deliveries: [] }),
      "Die Zustellnachweise sind derzeit nicht abrufbar.",
    ],
  ] as const)(
    "keeps missing, denied and unreadable channel evidence distinct",
    async (reply, expected) => {
      localStorage.setItem("mako-desk.rolle", "lieferant");
      billingd({}, [], reply);
      await open("/rechnung/b-1");
      expect(await screen.findByText(expected)).toBeTruthy();
      expect(screen.getByText(/Ausgestellt am/)).toBeTruthy();
    },
  );

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
