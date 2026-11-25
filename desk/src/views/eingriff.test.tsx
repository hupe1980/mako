import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// The Überblick's „Ohne Eingriff" tile: processd's count of the processes
// that ended with no person touching them, a share only where processd gives
// one (20 ended and up) — below that the count and what it is of.

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

function kpi(eingriff: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      const result =
        m.method === "tools/call" && m.params.name === "get_kpi_report"
          ? { structuredContent: { eingriff } }
          : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

const DEF = "Beendet ohne Freigabe, ohne Änderung und ohne Handgriff einer Person.";

describe("the Ohne-Eingriff tile", () => {
  it("shows a count and what it is of below 20 ended processes", async () => {
    kpi({
      beendet: 13,
      ohne_eingriff: 3,
      freigabe_unveraendert: 4,
      freigabe_geaendert: 2,
      person: 4,
      anteile: null,
      definition: DEF,
    });
    await open("/ueberblick");
    const tile = await screen.findByRole("link", { name: /^Ohne Eingriff/ });
    expect(await screen.findByText("3 von 13")).toBeTruthy();
    expect(tile.textContent).not.toContain("%");
    expect(tile.textContent).toContain(DEF);
  });

  it("shows processd's share from 20 ended processes up", async () => {
    kpi({
      beendet: 50,
      ohne_eingriff: 43,
      freigabe_unveraendert: 4,
      freigabe_geaendert: 2,
      person: 1,
      anteile: {
        ohne_eingriff: 0.86,
        freigabe_unveraendert: 0.08,
        freigabe_geaendert: 0.04,
        person: 0.02,
      },
      definition: DEF,
    });
    await open("/ueberblick");
    expect(await screen.findByText(/86\s?%/)).toBeTruthy();
    expect(await screen.findByText("43 von 50")).toBeTruthy();
  });
});

describe("the billing-deadline tile", () => {
  it("shows each Lieferant's overdue periods and what was billed on time and late, as billingd counts them", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
        if (url.startsWith("/svc/agentd/")) return Response.json([]);
        if (!init?.body) return Response.json({});
        const m = JSON.parse(String(init.body));
        const result =
          m.method === "tools/call" && m.params.name === "summarize_billing_deadlines"
            ? {
                structuredContent: {
                  lieferanten: [
                    {
                      lf_mp_id: "9900357000004",
                      owed: 13,
                      issued_on_time: 10,
                      issued_late: 1,
                      overdue: 2,
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
    await open("/ueberblick");
    const tile = await screen.findByRole("link", {
      name: /^Abrechnungen überfällig · 9900357000004/,
    });
    expect(tile.textContent).toContain(
      "13 geschuldet, 10 pünktlich ausgestellt, 1 verspätet ausgestellt",
    );
    expect(tile.textContent).toContain("2");
    localStorage.removeItem("mako-desk.rolle");
  });
});
