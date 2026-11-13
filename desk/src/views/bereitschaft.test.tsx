import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import checklisteJson from "../../fixtures/einrichtung-checkliste.json?raw";
import { RolleProvider } from "../app/rollen";
import { Bereitschaft, type Checkliste } from "./Einrichtung";

// The checklist renders makod's `einrichtung_checkliste` answer as it came:
// every colour, status and sentence is makod's. The fixture is makod's real
// answer for a planted deployment — `the_desk_fixture_is_makods_answer` in
// services/makod holds it to the service.

const LISTE = JSON.parse(checklisteJson) as Checkliste;

/** The Desk's runtime configuration, `/config.json`; absent unless a test sets it. */
let laufzeit: unknown = null;

/** makod answers `einrichtung_checkliste` with each of `antworten` in turn; calls are counted. */
function makod(...antworten: unknown[]) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit) => {
      if (url === "/config.json")
        return laufzeit === null ? new Response("", { status: 404 }) : Response.json(laufzeit);
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/call") {
        calls.push(`${url.split("/")[2]}/${m.params.name}`);
        const antwort = antworten[Math.min(calls.length, antworten.length) - 1];
        result = { structuredContent: antwort };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
  return calls;
}

function show() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RolleProvider>
        <Bereitschaft />
      </RolleProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  laufzeit = null;
  cleanup();
  vi.unstubAllGlobals();
});

describe("the readiness checklist", () => {
  it("shows every Bereich with makod's Ampel and every finding word for word", async () => {
    const calls = makod(LISTE);
    show();
    const punkte = await screen.findByRole("list", { name: "Punkte" });
    expect(calls).toEqual(["makod/einrichtung_checkliste"]);
    expect(within(punkte).getAllByRole("heading", { level: 4 })).toHaveLength(LISTE.punkte.length);
    expect(screen.getByText("Noch nicht bereit")).toBeTruthy();
    for (const p of LISTE.punkte) {
      for (const b of p.befunde) {
        expect(screen.getAllByText(b.text).length).toBeGreaterThan(0);
        if (b.pfad) expect(screen.getAllByText(b.pfad).length).toBeGreaterThan(0);
        if (b.quelle)
          expect(
            screen.getAllByRole("button", { name: `Quelle: ${b.quelle}` }).length,
          ).toBeGreaterThan(0);
      }
    }
    // The blocking identity finding sits under Identitäten, marked blocked.
    const identitaeten = within(punkte).getByRole("list", { name: "Befunde zu Identitäten" });
    expect(within(identitaeten).getByText(/blockiert:/)).toBeTruthy();
  });

  it("leads from an identity finding to the interview question that fixes it", async () => {
    makod(LISTE);
    const gesprungen: string[] = [];
    const hoeren = (e: Event) => gesprungen.push((e as CustomEvent<string>).detail);
    window.addEventListener("mako-desk:frage", hoeren);
    show();
    const punkte = await screen.findByRole("list", { name: "Punkte" });
    const identitaeten = within(punkte).getByRole("list", { name: "Befunde zu Identitäten" });
    fireEvent.click(
      within(identitaeten).getAllByRole("link", { name: /Im Vorschlag beheben/ })[0] as HTMLElement,
    );
    window.removeEventListener("mako-desk:frage", hoeren);
    expect(gesprungen).toEqual(["rollen"]);
    // A finding no interview question answers offers no such link.
    const erreichbarkeit = within(punkte).getByRole("list", { name: "Befunde zu Erreichbarkeit" });
    expect(within(erreichbarkeit).queryByRole("link", { name: /Im Vorschlag beheben/ })).toBeNull();
  });

  it("shows an unreachable service as unknown, not as failing", async () => {
    makod(LISTE);
    show();
    const dienste = await screen.findByRole("list", { name: "Dienste" });
    const messwerte = within(dienste).getByText("Messwerte").closest("li") as HTMLElement;
    expect(within(messwerte).getByText(/nicht erreichbar/)).toBeTruthy();
    expect(within(messwerte).queryByText(/blockiert/)).toBeNull();
    const erreichbarkeit = LISTE.punkte.find((p) => p.bereich === "erreichbarkeit");
    expect(erreichbarkeit?.ampel).not.toBe("gruen");
  });

  it("keeps an answer that is not a report verbatim", async () => {
    makod(LISTE);
    show();
    const roh = LISTE.berichte.find((b) => b.zustand === "unlesbar");
    expect(roh?.zustand).toBe("unlesbar");
    if (roh?.zustand !== "unlesbar") return;
    await screen.findByText("Antworten, die kein Bericht sind");
    const pre = screen
      .getAllByText((_, el) => el?.tagName === "PRE" && el.textContent === roh.roh)
      .at(0);
    expect(pre).toBeTruthy();
  });

  it("shows an answer of unknown shape as it came, never dropped", async () => {
    makod({ bereit: false, etwas: "Neues" });
    show();
    await screen.findByText(/unbekannte Form/);
    expect(screen.getByText(/"etwas": "Neues"/)).toBeTruthy();
  });

  it("links a setting's manual entry and names an unknown Bereich by makod's question", async () => {
    const liste: Checkliste = {
      ...LISTE,
      punkte: [
        {
          bereich: "kuenftig" as Checkliste["punkte"][number]["bereich"],
          frage: "Eine Frage, die der Desk noch nicht kennt",
          ampel: "gelb",
          befunde: [
            {
              dienst: "makod",
              schwere: "warnung",
              bereich: "konfiguration",
              text: "Eine Einstellung steht auf ihrem lockernden Wert.",
              pfad: "policy.as4.unbekannte_absender",
              referenz: "@/docs/services/makod.md#policy-as4-unbekannte-absender",
            },
          ],
        },
      ],
    };
    makod(liste);
    show();
    const link = await screen.findByRole("link", {
      name: "Handbuch: policy.as4.unbekannte_absender",
    });
    expect(link.getAttribute("href")).toBe(
      "https://hupe1980.github.io/mako/docs/services/makod/#policy-as4-unbekannte-absender",
    );
    expect(screen.getByText("Eine Frage, die der Desk noch nicht kennt")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "kuenftig", level: 4 })).toBeTruthy();
  });

  it("opens the manual under the address the runtime configuration names", async () => {
    laufzeit = { handbuch: "https://handbuch.example.org/mako/" };
    const liste: Checkliste = {
      ...LISTE,
      punkte: [
        {
          bereich: "konfiguration",
          frage: "",
          ampel: "gelb",
          befunde: [
            {
              dienst: "makod",
              schwere: "warnung",
              bereich: "konfiguration",
              text: "Eine Einstellung steht auf ihrem lockernden Wert.",
              pfad: "policy.as4.unbekannte_absender",
              referenz: "@/docs/services/makod.md#policy-as4-unbekannte-absender",
            },
          ],
        },
      ],
    };
    makod(liste);
    show();
    await waitFor(async () => {
      const link = await screen.findByRole("link", {
        name: "Handbuch: policy.as4.unbekannte_absender",
      });
      expect(link.getAttribute("href")).toBe(
        "https://handbuch.example.org/mako/docs/services/makod/#policy-as4-unbekannte-absender",
      );
    });
  });

  it("asks again on request and shows the new answer without a reload", async () => {
    const calls = makod(LISTE, { ...LISTE, bereit: true, punkte: [], berichte: [] });
    show();
    await screen.findByText("Noch nicht bereit");
    fireEvent.click(screen.getByRole("button", { name: /Jetzt prüfen/ }));
    await screen.findByText("Bereit für den Betrieb");
    await waitFor(() => expect(calls).toHaveLength(2));
  });
});
