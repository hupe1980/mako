import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { vergessen } from "../data/verweigert";
import { RolleProvider } from "./rollen";
import { createDeskRouter } from "./router";

// A held decision leaves with an Idempotency-Key, so a retry is one decision
// at the service; one sent on `pagehide`, whose answer the page did not live
// to read, is asked after on the next load — the service says what happened.

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
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  vergessen();
  localStorage.clear();
});

const freigabe = (status: string, decided_by: string | null) => ({
  id: "q-1",
  process_id: "p-1",
  pid: 55001,
  malo_id: "51238696012",
  marktrolle: "NB",
  grund: "Anmeldung eines verbundenen Lieferanten",
  art: "gehalten",
  antwort: { code: "E15", ebd: "E_0623", bedeutung: "Zustimmung" },
  codes_freigeben: null,
  codes_ablehnen: null,
  fakten_freigeben: [],
  fakten_ablehnen: [],
  kann_freigeben: true,
  kann_ablehnen: false,
  status,
  expires_at: "2031-01-02T09:00:00Z",
  created_at: "2031-01-01T09:02:00Z",
  decided_by,
  decided_at: decided_by ? "2031-01-01T09:10:00Z" : null,
});

/** processd lists one Freigabe; its decision door never answers (the page is gone). */
function stub(opts: { posts: { url: string; headers: Headers }[]; queueEntry?: unknown }) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.includes("/api/v1/queue/")) {
        opts.posts.push({ url, headers: new Headers(init?.headers) });
        return new Promise<Response>(() => undefined);
      }
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/call") {
        if (m.params.name === "list_pending_approvals")
          result = {
            structuredContent: {
              freigaben: [freigabe("Pending", null)],
              count: 1,
              saturated: false,
            },
          };
        else if (m.params.name === "get_queue_entry" && opts.queueEntry)
          result = { structuredContent: opts.queueEntry };
        else
          result = { structuredContent: { process_id: "p-1", workflow_name: "w", eintraege: [] } };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("a held decision's delivery", () => {
  it("a_decision_flushed_on_pagehide_shows_its_outcome_next_load", async () => {
    const posts: { url: string; headers: Headers }[] = [];
    stub({ posts });
    await open("/freigaben");
    fireEvent.click(await screen.findByRole("radio", { name: "Freigeben" }));
    fireEvent.click(screen.getByRole("button", { name: "Freigeben" }));
    // The tab closes inside the hold: the decision leaves with keepalive.
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() => expect(posts).toHaveLength(1));
    const key = posts[0]?.headers.get("idempotency-key");
    expect(key).toMatch(/^[0-9a-f-]{36}$/);

    // The next load asks processd what became of it, and says so.
    cleanup();
    stub({ posts, queueEntry: freigabe("Approved", "anna") });
    await open("/heute");
    const hinweis = await screen.findByRole("region", { name: "Beim letzten Mal gesendet" });
    expect(await within(hinweis).findByText(/freigegeben von anna/)).toBeTruthy();
    fireEvent.click(within(hinweis).getByRole("button", { name: "Verstanden" }));
    expect(screen.queryByRole("region", { name: "Beim letzten Mal gesendet" })).toBeNull();
    expect(localStorage.getItem("mako-desk.unbestaetigt")).toBe("[]");
  });
});
