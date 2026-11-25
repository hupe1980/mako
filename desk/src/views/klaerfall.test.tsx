import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import { vergessen } from "../data/verweigert";
import { katalog, ladeSprache, setSprache } from "../i18n";

// A Klärfall as processd keeps it: the cause in words, where it came from,
// when it is due, what happened to it — and one action: „Korrigieren &
// senden" where processd proposes a correction, else „Erledigt". Every
// action is held for undo and leaves with an Idempotency-Key.

async function open(path: string, rolle = "lieferant") {
  localStorage.setItem("mako-desk.rolle", rolle);
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
  setSprache("de");
});

const fall = (korrektur: unknown) => ({
  id: "k-1",
  message_ref: "UTILMD-2031-0007",
  partner_mp_id: "9900357000004",
  process_id: "p-7",
  pid: 55001,
  owner_role: "LF",
  origins: ["aperak"],
  zustand: "open",
  ursache_code: "Z29",
  ursache_liste: "ERC",
  bedeutung: "Erforderliche Angabe für diesen Anwendungsfall fehlt",
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
  antwort?: Response | Error,
  fields: Record<string, unknown> = {},
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
        if (antwort instanceof Error) throw antwort;
        return antwort ?? Response.json({});
      }
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      const result =
        m.method === "tools/call" && m.params.name === "get_klaerfall"
          ? {
              structuredContent: {
                klaerfall: { ...fall(korrektur), ...fields },
                verlauf: [
                  {
                    at: "2031-01-01T08:00:00Z",
                    akteur: "system",
                    schritt: "eroeffnet",
                    vorgeschlagen: "gpke.kuendigung.bestaetigen",
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

const vorschlag = {
  befehl: "gpke.kuendigung.bestaetigen",
  erlaubt: ["gpke.kuendigung.bestaetigen", "gpke.kuendigung.ablehnen"],
};

const flush = () =>
  act(() => {
    window.dispatchEvent(new Event("pagehide"));
  });

describe("the Klärfall view", () => {
  it.each(["de", "en"] as const)(
    "undo in %s restores the initiating control and leaves zero writes after pagehide",
    async (language) => {
      await ladeSprache(language);
      setSprache(language);
      const t = katalog();
      const posts: { url: string; body: unknown; key: string | null }[] = [];
      stub(vorschlag, posts, undefined, {
        due_at: null,
        due_source: null,
        aktion: "future_action",
      });
      await open("/klaerfall/k-1");
      const button = await screen.findByRole("button", { name: t.klaerfall.korrigieren });
      expect(screen.getByText(t.klaerfall.fristUnbekannt)).toBeTruthy();
      expect(screen.getByText(t.klaerfall.aktionUnbekannt)).toBeTruthy();
      expect(screen.getByText("future_action").closest("details")?.open).toBe(false);
      expect(screen.getByText(t.klaerfall.rolle, { exact: false }).textContent).toContain(
        t.rollenNamen.lieferant,
      );
      fireEvent.click(button);
      expect(screen.getByText(t.klaerfall.vorgemerkt).closest('[aria-live="polite"]')).toBeTruthy();
      expect(screen.queryByText(t.klaerfall.gesendet)).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: new RegExp(t.halten.rueckgaengig) }));
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: t.klaerfall.korrigieren }),
      );
      expect(screen.getByText(t.klaerfall.zurueckgenommen).getAttribute("role")).toBe("status");
      flush();
      expect(posts).toHaveLength(0);
    },
  );

  it.each(["de", "en"] as const)(
    "keeps the reason and focuses Details for an unknown refusal in %s",
    async (language) => {
      await ladeSprache(language);
      setSprache(language);
      const t = katalog();
      const posts: { url: string; body: unknown; key: string | null }[] = [];
      stub(
        vorschlag,
        posts,
        Response.json({ code: "future_refusal", detail: "raw diagnostic" }, { status: 422 }),
      );
      await open("/klaerfall/k-1");
      fireEvent.click(
        await screen.findByRole("radio", { name: language === "de" ? "Ablehnen" : "Reject" }),
      );
      fireEvent.change(screen.getByRole("textbox"), { target: { value: "Rücksprache mit Kunde" } });
      fireEvent.click(screen.getByRole("button", { name: t.klaerfall.korrigieren }));
      flush();
      const fallback = await screen.findByText(t.refusal.unknown);
      const refusal = fallback.closest('[role="status"]') as HTMLElement;
      expect(within(refusal).getByText(t.refusal.details)).toBe(document.activeElement);
      expect(
        within(refusal).getByText("future_refusal: raw diagnostic").closest("details")?.open,
      ).toBe(false);
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "Rücksprache mit Kunde",
      );
      expect(posts).toHaveLength(1);
    },
  );

  it.each([
    new Error("connection reset"),
    Response.json({ detail: "gateway timeout" }, { status: 504 }),
  ])("leaves an uncertain dispatch outcome explicit", async (error) => {
    stub(vorschlag, [], error);
    await open("/klaerfall/k-1");
    fireEvent.click(await screen.findByRole("button", { name: katalog().klaerfall.korrigieren }));
    flush();
    expect(await screen.findByText(katalog().klaerfall.unbekannt)).toBeTruthy();
    expect(screen.queryByText(katalog().klaerfall.gesendet)).toBeNull();
    expect(screen.queryByText(katalog().refusal.unknown)).toBeNull();
    expect(screen.queryByRole("button", { name: katalog().klaerfall.korrigieren })).toBeNull();
  });
  it.each(["open", "dispatched"] as const)(
    "reads a %s case after a lost response without repeating or attributing the write",
    async (zustand) => {
      const posts: { url: string; body: unknown; key: string | null }[] = [];
      stub(vorschlag, posts, new TypeError("connection interrupted"));
      await open("/klaerfall/k-1");
      fireEvent.click(await screen.findByRole("button", { name: katalog().klaerfall.korrigieren }));
      flush();
      await screen.findByText(katalog().klaerfall.unbekannt);
      const storage = localStorage.getItem("mako-desk.unbestaetigt");
      expect(
        JSON.parse(storage ?? "[]").map((item: { schluessel: string }) => item.schluessel),
      ).toEqual([posts[0]?.key]);
      cleanup();
      stub(vorschlag, posts, undefined, { zustand, decided_by: "other operator" });
      await open("/heute", "netz");
      expect(screen.queryByRole("region", { name: katalog().unbestaetigt.titel })).toBeNull();
      expect(localStorage.getItem("mako-desk.unbestaetigt")).toBe(storage);
      cleanup();
      await open("/heute");
      const recovery = await screen.findByRole("region", { name: katalog().unbestaetigt.titel });
      await within(recovery).findByRole("link", { name: /UTILMD-2031-0007/ });
      expect(recovery.textContent).toContain(katalog().unbestaetigt.fallUnbekannt);
      expect(recovery.textContent).toContain(
        katalog().unbestaetigt.fallStand(katalog().klaerfall.zustand[zustand]),
      );
      expect(within(recovery).getAllByRole("button")).toHaveLength(1);
      fireEvent.click(
        within(recovery).getByRole("button", { name: katalog().unbestaetigt.verstanden }),
      );
      expect(posts).toHaveLength(1);
      expect(localStorage.getItem("mako-desk.unbestaetigt")).toBe("[]");
    },
  );

  it("reads as its cause, and „Korrigieren & senden“ sends the proposed correction, held for undo", async () => {
    const posts: { url: string; body: unknown; key: string | null }[] = [];
    stub(
      { befehl: "gpke.kuendigung.bestaetigen", erlaubt: ["gpke.kuendigung.bestaetigen"] },
      posts,
    );
    await open("/klaerfall/k-1");
    expect(
      await screen.findByRole("heading", {
        level: 1,
        name: /Erforderliche Angabe für diesen Anwendungsfall fehlt/,
      }),
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
        befehl: "gpke.kuendigung.bestaetigen",
        erlaubt: ["gpke.kuendigung.bestaetigen", "gpke.kuendigung.ablehnen"],
      },
      posts,
    );
    await open("/klaerfall/k-1");
    fireEvent.click(await screen.findByRole("radio", { name: "Ablehnen" }));
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
      befehl: "gpke.kuendigung.ablehnen",
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
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("abgeschlossen");
    expect(screen.queryByRole("button", { name: "Erledigt" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Zuweisen" })).toBeNull();
    const reason = screen.getByRole("textbox") as HTMLTextAreaElement;
    expect(reason.value).toBe("telefonisch geklärt");
    expect(reason.readOnly).toBe(true);
  });
});
