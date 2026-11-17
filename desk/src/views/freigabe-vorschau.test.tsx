import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import { vergessen } from "../data/verweigert";

// A Freigabe shows what approving would send: processd's rehearsal of the
// decision, rendered by makod's door with the AHB names; the approval names
// that body's hash. The assistant's proposal is taken in unchanged only where
// the gate passed it.

const PROBE = JSON.parse(
  readFileSync(resolve(import.meta.dirname, "../../fixtures/probe-44004-abgelehnt.json"), "utf8"),
);

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

const freigabe = {
  id: "q-1",
  process_id: "p-1",
  pid: 44001,
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
  status: "Pending",
  expires_at: "2031-01-02T09:00:00Z",
  created_at: "2031-01-01T09:02:00Z",
  decided_by: null,
  decided_at: null,
};

const proposal = (urteil: string) => ({
  run: "r-1",
  status: "succeeded",
  disclosure: "AI-generated.",
  saetze: [],
  vorschlag: {
    aktion: "freigeben",
    antwortcode: null,
    begruendung: "Kein Prüfschritt schlägt an.",
    belegt: true,
  },
  pruefvermerk: {
    urteil,
    stufen: [
      {
        stufe: "entscheidung",
        urteil,
        code: urteil === "bestanden" ? null : "code_outside_tree",
        regel: "E_0623",
        segment: null,
        text: null,
      },
    ],
    grund: null,
    body_sha256: "abc123",
    geprueft_um: "2031-01-01T09:03:00Z",
  },
  schritte: [],
  hilfreich: null,
});

function stub(
  posts: { url: string; body: unknown; type: string | null }[],
  wartend: unknown = null,
  opts: { probeAbgelehnt?: boolean; entscheidung?: Response } = {},
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.includes("/api/v1/queue/")) {
        posts.push({
          url,
          body: JSON.parse(String(init?.body)),
          type: new Headers(init?.headers).get("content-type"),
        });
        return opts.entscheidung ?? new Response(null, { status: 200 });
      }
      if (url.startsWith("/svc/makod/api/v1/probe/"))
        return opts.probeAbgelehnt
          ? Response.json(
              { error: "Unprocessable Entity", code: "not_rehearsable", detail: "cannot rehearse" },
              { status: 422 },
            )
          : Response.json(PROBE);
      if (url.startsWith("/svc/agentd/api/v1/vorschlaege"))
        return Response.json({ freigabe: wartend ? { "q-1": wartend } : {} });
      if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/call") {
        if (m.params.name === "list_pending_approvals")
          result = { structuredContent: { freigaben: [freigabe], count: 1, saturated: false } };
        else if (m.params.name === "rehearse_decision")
          result = {
            structuredContent: {
              id: "q-1",
              freigeben: true,
              urteil: "besteht",
              command: "geli.lieferbeginn.antworten",
              marktrolle: "NB",
              payload: { malo_id: "51238696012", antwortcode: "E15" },
              body_sha256: "abc123",
              grund: null,
            },
          };
        else
          result = { structuredContent: { process_id: "p-1", workflow_name: "w", eintraege: [] } };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("a Freigabe's rendering", () => {
  it("shows the message approving would send, and approves that body by its hash", async () => {
    const posts: { url: string; body: unknown; type: string | null }[] = [];
    stub(posts);
    await open("/freigaben");
    fireEvent.click(await screen.findByRole("radio", { name: "Freigeben" }));
    // processd rehearsed the decision; makod rendered it with its AHB names.
    const vorschau = await screen.findByRole("region", { name: "Was gesendet wird" });
    expect(within(vorschau).getByText(/nimmt diese Entscheidung so an/)).toBeTruthy();
    expect(
      await within(vorschau).findByRole("article", { name: /UTILMD/ }, { timeout: 4000 }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Freigeben" }));
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0]?.type).toBe("application/json");
    expect(posts[0]?.body).toMatchObject({ body_sha256: "abc123" });
  });

  it("offers no take-in for a proposal the gate did not pass, and says why", async () => {
    stub([], proposal("nicht_geprueft"));
    await open("/freigaben");
    await screen.findByText(/Kein Prüfschritt schlägt an/);
    expect(screen.queryByRole("button", { name: "In die Entscheidung übernehmen" })).toBeNull();
    expect(screen.getByText(/Nicht geprüft/)).toBeTruthy();
  });

  it("takes in a proposal the gate passed for the body shown", async () => {
    stub([], proposal("bestanden"));
    await open("/freigaben");
    fireEvent.click(await screen.findByRole("button", { name: "In die Entscheidung übernehmen" }));
    const radio = (await screen.findByRole("radio", { name: "Freigeben" })) as HTMLInputElement;
    await waitFor(() => expect(radio.checked).toBe(true));
  });

  it("an_unrenderable_entry_offers_no_approve_unchanged", async () => {
    stub([], proposal("bestanden"), { probeAbgelehnt: true });
    await open("/freigaben");
    await screen.findByText(/Kein Prüfschritt schlägt an/);
    // The gate passed it, but makod cannot render what approving sends: the
    // proposal is not taken in unchanged.
    expect(await screen.findByText(/nicht vorab darstellen/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "In die Entscheidung übernehmen" })).toBeNull();
  });

  it("renders processd's refusal of an unchecked decision in the catalogue's words", async () => {
    stub([], null, {
      entscheidung: Response.json(
        {
          error: "Unprocessable Entity",
          code: "pruefung_abgelehnt",
          detail: "the decision failed its check",
          regeln: ["E_0623"],
          stufe: "entscheidung",
          grund: "code_outside_tree",
        },
        { status: 422 },
      ),
    });
    await open("/freigaben");
    fireEvent.click(await screen.findByRole("radio", { name: "Freigeben" }));
    await screen.findByRole("region", { name: "Was gesendet wird" });
    fireEvent.click(screen.getByRole("button", { name: "Freigeben" }));
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(await screen.findByText(/Die Prüfung hat diese Entscheidung abgelehnt/)).toBeTruthy();
    expect(screen.getByText(/E_0623/)).toBeTruthy();
  });
});
