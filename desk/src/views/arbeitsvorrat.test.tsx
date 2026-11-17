import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// Heute's Arbeitsvorrat: one row per source the Rolle reads, its count and its
// oldest item, a zero kept visible — and a source that did not answer said
// so, never shown as 0.

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
});

const freigabe = (id: string, created_at: string) => ({
  id,
  process_id: `p-${id}`,
  pid: 55001,
  malo_id: "51238696012",
  marktrolle: "NB",
  grund: `Anmeldung ${id}`,
  art: "eskaliert",
  antwort: null,
  codes_freigeben: null,
  codes_ablehnen: null,
  fakten_freigeben: [],
  fakten_ablehnen: [],
  kann_freigeben: true,
  kann_ablehnen: true,
  status: "Pending",
  expires_at: "2031-01-02T09:00:00Z",
  created_at,
  decided_by: null,
  decided_at: null,
});

function stub() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      // invoicd does not answer at all.
      if (url.startsWith("/svc/invoicd/")) return new Response("down", { status: 502 });
      let result: unknown = {};
      if (m.method === "tools/call") {
        const tool = `${url.split("/")[2]}/${m.params.name}`;
        if (tool === "processd/list_pending_approvals")
          result = {
            structuredContent: {
              // processd answers by Marktrolle: these wait on the NB alone.
              freigaben:
                m.params.arguments?.marktrolle === "NB"
                  ? [freigabe("b", "2026-10-01T08:00:00Z"), freigabe("a", "2026-09-22T08:00:00Z")]
                  : [],
              count: 2,
              saturated: false,
            },
          };
        else if (tool === "processd/list_klaerfaelle")
          result = {
            structuredContent: {
              count: 1,
              saturated: false,
              klaerfaelle: [
                {
                  id: "k-1",
                  message_ref: "UTILMD-2031-0007",
                  process_id: "p-7",
                  pid: 55001,
                  origins: ["aperak"],
                  ursache_code: "Z29",
                  ursache_liste: "APERAK",
                  bedeutung: "Prüfidentifikator unbekannt",
                  aktion: "retry_with_correction",
                  hinweis: "resend with a corrected PID",
                  befehl: null,
                  due_at: "2031-01-03T10:00:00Z",
                  due_source: "APERAK AHB 1.1 Kap. 2.4.1",
                  overdue: false,
                  occurrences: 2,
                  opened_at: "2031-01-01T08:00:00Z",
                },
              ],
            },
          };
        else if (tool === "netzbilanzd/list_korrekturbedarf")
          result = { structuredContent: { korrekturbedarf: [] } };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

const row = (list: HTMLElement, name: RegExp) =>
  within(list)
    .getAllByRole("listitem")
    .find((li) => name.test(li.textContent ?? "")) as HTMLElement;

describe("the Arbeitsvorrat on Heute", () => {
  it("shows each source's count and oldest item, a zero kept, a silent source unavailable", async () => {
    stub();
    await open("/heute");
    const list = await screen.findByRole("list", { name: "Arbeitsvorrat" });
    await waitFor(() => expect(row(list, /^Freigaben/)?.textContent).toContain("2"));
    expect(row(list, /^Freigaben/).textContent).toContain("älteste vom 22.09.2026");
    // Nothing to correct is a row too, with its 0; Klärfälle stand in for
    // lapsed Fristen and dead letters, which have no row of their own.
    await waitFor(() => expect(row(list, /^Korrekturbedarf/)?.textContent).toMatch(/0$/));
    await waitFor(() => expect(row(list, /^Klärfälle/)?.textContent).toMatch(/1$/));
    expect(row(list, /^Überfällige Prozesse/)).toBeUndefined();
    // A Klärfall reads as its cause in the list's words and what to do about it.
    const fall = (await screen.findByText(/Prüfidentifikator unbekannt/)).closest("li");
    expect(fall?.textContent).toContain("Korrigieren und neu senden");
    expect(fall?.textContent).toContain("UTILMD-2031-0007");
    // invoicd did not answer: unavailable, never 0.
    await waitFor(() =>
      expect(row(list, /^Gehaltene Rechnungen/)?.textContent).toMatch(/nicht erreichbar/),
    );
    expect(row(list, /^Gehaltene Rechnungen/).textContent).not.toMatch(/0$/);
  });

  it("opens a source's own list from its row", async () => {
    stub();
    const router = await open("/heute");
    const list = await screen.findByRole("list", { name: "Arbeitsvorrat" });
    await waitFor(() => expect(row(list, /^Freigaben/)?.textContent).toContain("2"));
    fireEvent.click(within(row(list, /^Freigaben/)).getByRole("link"));
    await waitFor(() => expect(router.state.location.pathname).toBe("/freigaben"));
  });

  it("lists the Lieferant's unbilled periods by the date the service names, with its days left", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url.startsWith("/svc/agentd/")) return Response.json([]);
        if (!init?.body) return Response.json({});
        const m = JSON.parse(String(init.body));
        let result: unknown = {};
        if (m.method === "tools/call" && m.params.name === "list_unbilled_periods")
          result = {
            structuredContent: {
              periods: [
                {
                  malo_id: "51238696012",
                  lf_mp_id: "9900357000004",
                  period_from: "2030-01-01",
                  period_to: "2030-12-31",
                  final_invoice: true,
                  reason_code: "NO_METER_DATA",
                  message: "no meter reading for the period",
                  due_on: "2031-01-21",
                  days_left: -2,
                  attempts: 3,
                  settled_by: null,
                },
              ],
              count: 1,
              saturated: false,
            },
          };
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/heute");
    const list = await screen.findByRole("list", { name: "Arbeitsvorrat" });
    await waitFor(() => expect(row(list, /^Offene Abrechnungen/)?.textContent).toMatch(/1$/));
    // The row's reason is the catalogue's sentence for the service's code; the
    // age is the service's days_left, said as such.
    const item = (await screen.findByText(/Für den Zeitraum fehlen Messwerte/)).closest("li");
    expect(item?.textContent).toContain("Schlussrechnung");
    expect(item?.textContent).toContain("fällig am 21.01.2031");
    expect(item?.textContent).toContain("seit 2 Tagen überfällig");
    localStorage.removeItem("mako-desk.rolle");
  });
});

describe("a Rolle switch (SR-046)", () => {
  it("leaves no row of the Rolle left on screen", async () => {
    stub();
    await open("/heute");
    // A Netz Freigabe is on screen.
    expect((await screen.findAllByText("Anmeldung a")).length).toBeGreaterThan(0);
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "2", ctrlKey: true }));
    });
    await waitFor(() => expect(document.documentElement.dataset.rolle).toBe("lieferant"));
    // None of the Netz Rolle's data stays in the DOM after the switch.
    await waitFor(() => expect(screen.queryAllByText("Anmeldung a")).toHaveLength(0));
    expect(document.body.textContent).not.toContain("Anmeldung b");
  });
});
