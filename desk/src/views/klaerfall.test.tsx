import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import { vergessen } from "../data/verweigert";

// A Klärfall as processd keeps it: the cause in words, where it came from,
// when it is due, what happened to it — and one action: „Korrigieren &
// senden" where processd proposes a correction, else „Erledigt". Every
// action is held for undo and leaves with an Idempotency-Key.

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
  vergessen();
  localStorage.clear();
});

const fall = (korrektur: unknown) => ({
  id: "k-1",
  message_ref: "UTILMD-2031-0007",
  partner_mp_id: "9900357000004",
  process_id: "p-7",
  pid: 55001,
  owner_role: "NB",
  origins: ["aperak"],
  zustand: "open",
  ursache_code: "Z29",
  ursache_liste: "APERAK",
  bedeutung: "Prüfidentifikator unbekannt",
  aktion: "retry_with_correction",
  hinweis: "resend with a corrected PID",
  korrektur,
  due_at: "2031-01-03T10:00:00Z",
  due_source: "APERAK AHB 1.1 Kap. 2.4.1",
  overdue: false,
  operator_window_ends_at: null,
  assignee: null,
  decided_by: null,
  occurrences: 2,
  opened_at: "2031-01-01T08:00:00Z",
  closed_at: null,
});

function stub(
  korrektur: unknown,
  posts: { url: string; body: unknown; key: string | null }[],
  antwort?: Response,
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.startsWith("/svc/processd/api/v1/klaerfaelle/")) {
        posts.push({
          url,
          body: JSON.parse(String(init?.body)),
          key: new Headers(init?.headers).get("idempotency-key"),
        });
        return antwort ?? Response.json({});
      }
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      const result =
        m.method === "tools/call" && m.params.name === "get_klaerfall"
          ? {
              structuredContent: {
                klaerfall: fall(korrektur),
                verlauf: [
                  {
                    at: "2031-01-01T08:00:00Z",
                    akteur: "system",
                    schritt: "eroeffnet",
                    vorgeschlagen: "gpke.anmeldung.korrigieren",
                    befehl: null,
                    begruendung: null,
                    assignee: null,
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

const flush = () =>
  act(() => {
    window.dispatchEvent(new Event("pagehide"));
  });

describe("the Klärfall view", () => {
  it("reads as its cause, and „Korrigieren & senden“ sends the proposed correction, held for undo", async () => {
    const posts: { url: string; body: unknown; key: string | null }[] = [];
    stub({ befehl: "gpke.anmeldung.korrigieren", erlaubt: ["gpke.anmeldung.korrigieren"] }, posts);
    await open("/klaerfall/k-1");
    expect(
      await screen.findByRole("heading", { level: 1, name: /Prüfidentifikator unbekannt/ }),
    ).toBeTruthy();
    const bar = screen.getByRole("region", { name: "Wartet auf deine Entscheidung" });
    const knoepfe = within(bar)
      .getAllByRole("button")
      .filter((b) => !b.getAttribute("aria-label")?.startsWith("Quelle"));
    expect(knoepfe.map((b) => b.textContent)).toEqual(["Korrigieren & senden"]);
    fireEvent.click(knoepfe[0] as HTMLElement);
    expect(posts).toEqual([]);
    flush();
    await waitFor(() =>
      expect(posts.map((p) => p.url)).toEqual(["/svc/processd/api/v1/klaerfaelle/k-1/dispatch"]),
    );
    expect(posts[0]?.body).toEqual({});
    expect(posts[0]?.key).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("asks a reason for another command than the proposal", async () => {
    const posts: { url: string; body: unknown; key: string | null }[] = [];
    stub(
      {
        befehl: "gpke.anmeldung.korrigieren",
        erlaubt: ["gpke.anmeldung.korrigieren", "gpke.anmeldung.stornieren"],
      },
      posts,
    );
    await open("/klaerfall/k-1");
    fireEvent.click(await screen.findByRole("radio", { name: "gpke.anmeldung.stornieren" }));
    const senden = screen.getByRole("button", { name: "Korrigieren & senden" });
    expect(senden.hasAttribute("disabled") || senden.getAttribute("data-disabled") === "true").toBe(
      true,
    );
    fireEvent.change(screen.getByRole("textbox", { name: /Begründung/ }), {
      target: { value: "Kunde zieht zurück" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Korrigieren & senden" }));
    flush();
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0]?.body).toEqual({
      befehl: "gpke.anmeldung.stornieren",
      begruendung: "Kunde zieht zurück",
    });
  });

  it("offers „Erledigt“ with a reason where no correction is proposed, and says a refusal in words", async () => {
    const posts: { url: string; body: unknown; key: string | null }[] = [];
    stub(
      null,
      posts,
      Response.json({ code: "case_closed", detail: "the case is closed" }, { status: 409 }),
    );
    await open("/klaerfall/k-1");
    fireEvent.change(await screen.findByRole("textbox", { name: /Begründung/ }), {
      target: { value: "telefonisch geklärt" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Erledigt" }));
    flush();
    await waitFor(() =>
      expect(posts.map((p) => p.url)).toEqual(["/svc/processd/api/v1/klaerfaelle/k-1/resolve"]),
    );
    expect(posts[0]?.body).toEqual({ begruendung: "telefonisch geklärt" });
    expect(await screen.findByText(/Dieser Klärfall ist schon abgeschlossen/)).toBeTruthy();
  });
});
