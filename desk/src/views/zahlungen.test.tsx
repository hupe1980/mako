import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// Zahlungen: each payment no account took, with accountingd's proposal and
// one key to accept it; each Guthaben due with „Ausgezahlt". Held for undo,
// sent with an Idempotency-Key; a lost race is said in accountingd's code.

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

function stub(posts: { url: string; body: unknown; key: string | null }[], assign?: Response) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/accountingd/api/v1/payments/unmatched"))
        return Response.json({
          count: 1,
          saturated: false,
          payments: [
            {
              payment_id: "pay-1",
              amount_ct: 4210,
              amount_eur: "42.10",
              bank_transaction_id: "b-1",
              end_to_end_id: null,
              iban_masked: "DE89 •••• 3000",
              is_return: false,
              received_at: "2031-01-02T08:00:00Z",
              reference: "Kd 10042 Abschlag",
              source: "camt053",
              value_date: "2031-01-02",
            },
          ],
        });
      if (url === "/svc/accountingd/api/v1/payments/match")
        return Response.json({
          candidates: [
            {
              account_id: "acc-1",
              confidence: "hoch",
              lf_mp_id: "9905550000010",
              malo_id: "51238696012",
              matched_by: "reference",
              open_balance_ct: 4210,
              open_balance_eur: "42.10",
              residual_ct: 0,
              similarity_score: 1,
            },
          ],
          candidates_count: 1,
          note: "",
          payment_amount_ct: 4210,
          payment_amount_eur: "42.10",
          payment_reference: "Kd 10042 Abschlag",
          tolerance_pct: 0,
        });
      if (url.includes("/assign") || url.includes("/paid")) {
        posts.push({
          url,
          body: JSON.parse(String(init?.body)),
          key: new Headers(init?.headers).get("idempotency-key"),
        });
        return assign ?? Response.json({});
      }
      if (url.startsWith("/svc/accountingd/api/v1/credit-payouts"))
        return Response.json({ count: 0, overdue_count: 0, saturated: false, payouts: [] });
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: {} }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("Zahlungen", () => {
  it("assigns a payment to accountingd's proposal with one key, held for undo", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown; key: string | null }[] = [];
    stub(posts);
    await open("/zahlungen");
    const liste = await screen.findByRole("list", { name: "Zahlungen ohne Zuordnung" });
    expect(await within(liste).findByText(/Vorschlag: Konto der MaLo 51238696012/)).toBeTruthy();
    expect(within(liste).getByText(/DE89 •••• 3000/)).toBeTruthy();
    fireEvent.keyDown(within(liste).getAllByRole("listitem")[0] as HTMLElement, { key: "a" });
    expect(posts).toEqual([]);
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() =>
      expect(posts.map((p) => [p.url, p.body])).toEqual([
        [
          "/svc/accountingd/api/v1/payments/pay-1/assign",
          { malo_id: "51238696012", lf_mp_id: "9905550000010" },
        ],
      ]),
    );
    expect(posts[0]?.key).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("says a payment someone else assigned first in accountingd's words", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    stub(
      [],
      Response.json({ code: "payment_already_assigned", detail: "already" }, { status: 409 }),
    );
    await open("/zahlungen");
    fireEvent.click(await screen.findByRole("button", { name: "Zuordnen" }));
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(await screen.findByText("Diese Zahlung hat schon jemand zugeordnet.")).toBeTruthy();
  });
});
