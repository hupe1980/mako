import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

// Zustellungen: a service's webhook endpoints, a new one's signing secret
// shown once, a test event, the deliveries with their status and latency, and
// a replay — the service's own subscription API, nothing kept in the Desk.

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

const ep = (id: string, owner: "api" | "config") => ({
  id,
  name: owner === "config" ? "ERP (Konfiguration)" : "Portal",
  url: `https://erp.example/${id}`,
  owner,
  active: true,
  event_types: ["de.mako.messwerte.eingegangen"],
  extensions: {},
  created_at: "2026-11-01T08:00:00Z",
  deactivated_reason: null,
  partner: null,
  previous_secret_expires_at: null,
  version: 1,
});

function stub(posts: { url: string; body: unknown }[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (init?.method === "POST" && url.startsWith("/svc/edmd/api/v1/webhooks/")) {
        posts.push({ url, body: init.body ? JSON.parse(String(init.body)) : null });
        if (url.endsWith("/webhooks/endpoints"))
          return Response.json(
            { endpoint: ep("ep-neu", "api"), secret: "whsec_EINMAL" },
            { status: 201 },
          );
        if (url.endsWith("/test"))
          return Response.json({ delivered: true, status: 200, latency_ms: 84, error: null });
        return Response.json({ queued: 1 });
      }
      if (url.startsWith("/svc/edmd/api/v1/webhooks/endpoints/ep-1/deliveries"))
        return Response.json({
          items: [
            {
              id: "d-1",
              endpoint_id: "ep-1",
              event_id: "ev-1",
              event_type: "de.mako.messwerte.eingegangen",
              status: "dead_lettered",
              attempts: 8,
              last_status: 503,
              latency_ms: 2100,
              last_error: "service unavailable",
              created_at: "2026-11-03T08:00:00Z",
              next_attempt_at: null,
              ordering_key: null,
            },
          ],
          next_cursor: null,
          total_estimate: null,
        });
      if (url.startsWith("/svc/edmd/api/v1/webhooks/endpoints"))
        return Response.json({
          items: [ep("ep-1", "api"), ep("ep-cfg", "config")],
          next_cursor: null,
          total_estimate: null,
        });
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: {} }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("Zustellungen", () => {
  it("lists a service's endpoints, the configured one read-only, and an endpoint's deliveries with status and latency", async () => {
    stub([]);
    await open("/zustellungen?dienst=edmd");
    const liste = await screen.findByRole("list", { name: "Endpunkte" });
    const zeilen = await within(liste).findAllByRole("listitem");
    expect(zeilen).toHaveLength(2);
    expect(zeilen[1]?.textContent).toContain("aus der Konfiguration");
    fireEvent.click(within(zeilen[0] as HTMLElement).getByRole("button", { name: /Portal/ }));
    const tabelle = await screen.findByRole("list", { name: "Zustellungen" });
    await within(tabelle).findAllByRole("listitem");
    expect(tabelle.textContent).toContain("503");
    expect(tabelle.textContent).toContain("2.100 ms");
    expect(tabelle.textContent).toContain("verworfen");
  });

  it("shows a new endpoint's secret once, with copy", async () => {
    const posts: { url: string; body: unknown }[] = [];
    stub(posts);
    await open("/zustellungen?dienst=edmd");
    await within(await screen.findByRole("list", { name: "Endpunkte" })).findAllByRole("listitem");
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), {
      target: { value: "Portal 2" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "URL" }), {
      target: { value: "https://portal.example/hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Endpunkt anlegen" }));
    expect(await screen.findByText("whsec_EINMAL")).toBeTruthy();
    expect(screen.getByText(/siehst du nur jetzt/)).toBeTruthy();
    expect(posts[0]?.body).toEqual({ name: "Portal 2", url: "https://portal.example/hook" });
  });

  it("sends a test event and shows its status code and latency", async () => {
    const posts: { url: string; body: unknown }[] = [];
    stub(posts);
    await open("/zustellungen?dienst=edmd");
    const liste = await screen.findByRole("list", { name: "Endpunkte" });
    fireEvent.click(
      (await within(liste).findAllByRole("button", { name: /Portal/ }))[0] as HTMLElement,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Testereignis senden" }));
    expect(await screen.findByText(/zugestellt · 200 · 84 ms/)).toBeTruthy();
    expect(posts.map((p) => p.url)).toContain("/svc/edmd/api/v1/webhooks/endpoints/ep-1/test");
  });

  // A dead-lettered delivery is replayed from its own row.
  it("a failed delivery is replayed from its row", async () => {
    const posts: { url: string; body: unknown }[] = [];
    stub(posts);
    await open("/zustellungen?dienst=edmd");
    const liste = await screen.findByRole("list", { name: "Endpunkte" });
    fireEvent.click(
      (await within(liste).findAllByRole("button", { name: /Portal/ }))[0] as HTMLElement,
    );
    const zeile = (
      await within(await screen.findByRole("list", { name: "Zustellungen" })).findByText(
        /verworfen/,
      )
    ).closest("li") as HTMLElement;
    fireEvent.click(within(zeile).getByRole("button", { name: "Erneut senden" }));
    await waitFor(() =>
      expect(posts.map((p) => p.url)).toContain("/svc/edmd/api/v1/webhooks/deliveries/d-1/replay"),
    );
  });
});
