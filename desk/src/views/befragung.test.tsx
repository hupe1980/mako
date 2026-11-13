import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import vorschlagJson from "../../fixtures/einrichtung-vorschlag.json?raw";
import vorbelegenJson from "../../fixtures/vorbelegen.json?raw";
import { RolleProvider } from "../app/rollen";
import { Befragung, type Vorschlag } from "./Befragung";

// The interview asks; makod's `einrichtung_vorschlag` builds the proposal. The
// fixture is makod's answer for the „Lieferant Strom und Gas“ interview
// against the planted deployment — `the_proposed_identities_pass_makods_own_checks`
// in services/makod holds it to the service. The pre-fill is agentd's run;
// every answer it reads is offered, never taken unasked.

const VORSCHLAG = JSON.parse(vorschlagJson) as Vorschlag;
/** agentd's answer to a pre-fill run — `a_sentence_is_read_into_answers…` in services/agentd holds it. */
const VORBELEGT = JSON.parse(vorbelegenJson) as { angaben: unknown };

interface Aufruf {
  tool: string;
  args: Record<string, unknown>;
}

/** makod answers every `einrichtung_vorschlag` with `antwort`; agentd as `agentd` says. */
function dienste(
  antwort: unknown,
  agentd: "weg" | { angaben: unknown } = "weg",
): { makod: Aufruf[]; agentd: string[] } {
  const makod: Aufruf[] = [];
  const agent: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/config.json") return new Response("", { status: 404 });
      if (url.startsWith("/svc/agentd")) {
        agent.push(`${init?.method ?? "GET"} ${url}`);
        if (agentd === "weg") throw new TypeError("Failed to fetch");
        if (init?.method === "POST")
          return Response.json([{ run_id: "r-1", agent_name: "einrichtung-assistent" }], {
            status: 202,
          });
        return Response.json({
          run: "r-1",
          status: "succeeded",
          disclosure: "AI-generated.",
          saetze: [],
          vorschlag: null,
          angaben: agentd.angaben,
          schritte: [],
          hilfreich: null,
        });
      }
      const m = JSON.parse(String(init?.body));
      let result: unknown = {};
      if (m.method === "tools/call") {
        makod.push({ tool: m.params.name, args: m.params.arguments });
        result = { structuredContent: antwort };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
  return { makod, agentd: agent };
}

function show() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RolleProvider>
        <Befragung />
      </RolleProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the setup interview", () => {
  it("starts from what the deployment runs with and shows makod's files line by line", async () => {
    const { makod } = dienste(VORSCHLAG);
    show();
    const tabs = await screen.findByRole("tablist", { name: "Dateien" });
    expect(
      within(tabs)
        .getAllByRole("tab")
        .map((t) => t.textContent),
    ).toEqual([expect.stringContaining("makod.toml"), expect.stringContaining("processd.toml")]);
    // The first ask carries no answer; the interview then starts from makod's `vorbelegt`.
    expect(makod[0]).toEqual({ tool: "einrichtung_vorschlag", args: { antworten: {} } });
    const gas = await screen.findByRole("checkbox", { name: "Netzbetreiber Gas" });
    expect((gas as HTMLInputElement).checked).toBe(true);
    expect(
      (screen.getByRole("textbox", { name: "MP-ID Netzbetreiber Gas" }) as HTMLInputElement).value,
    ).toBe("9900001000001");

    const zeilen = screen.getByRole("list", { name: "Zeilen von makod.toml" });
    const datei = VORSCHLAG.dateien[0];
    for (const z of datei?.zeilen ?? []) {
      expect(zeilen.textContent).toContain(z.text);
    }
    // Each kind of line says what it is in words, not by colour or sign alone.
    expect(within(zeilen).getAllByText("entfällt:").length).toBeGreaterThan(0);
    expect(within(zeilen).getAllByText("neu:").length).toBeGreaterThan(0);
    expect(within(zeilen).getAllByText("Lücke:").length).toBeGreaterThan(0);
    expect(screen.getByText("Noch nicht vollständig")).toBeTruthy();
  });

  it("copies the file to paste, the same text makod built", async () => {
    dienste(VORSCHLAG);
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "makod.toml kopieren" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(VORSCHLAG.dateien[0]?.toml));
    // A copied file holds no key material — the proposal names paths only.
    expect(VORSCHLAG.dateien.some((d) => d.toml.includes("BEGIN"))).toBe(false);
  });

  it("asks makod again with the answer the reader gives", async () => {
    const { makod } = dienste(VORSCHLAG);
    show();
    fireEvent.click(await screen.findByRole("checkbox", { name: "Lieferant Strom" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "MP-ID Lieferant Strom" }), {
      target: { value: "9900357000004" },
    });
    await waitFor(() =>
      expect(makod.at(-1)?.args).toEqual({
        antworten: expect.objectContaining({
          rollen: expect.arrayContaining([{ rolle: "lf-strom", mp_id: "9900357000004" }]),
        }),
      }),
    );
  });

  it("leads from a gap to the question that fills it", async () => {
    dienste(VORSCHLAG);
    show();
    fireEvent.click(await screen.findByRole("tab", { name: /processd\.toml/ }));
    const zeilen = await screen.findByRole("list", { name: "Zeilen von processd.toml" });
    const luecke = within(zeilen)
      .getAllByRole("listitem")
      .find((li) => li.dataset.art === "fehlt") as HTMLElement;
    fireEvent.click(within(luecke).getByRole("button", { name: "Zur Frage" }));
    await waitFor(() =>
      expect(document.activeElement?.closest("[data-frage]")?.getAttribute("data-frage")).toMatch(
        /^(verbundene_lieferanten|bilanzkreise|primaer|rollen)/,
      ),
    );
  });

  it("shows a refused answer with the rule it breaks", async () => {
    const abgelehnt: Vorschlag = {
      ...VORSCHLAG,
      abgelehnt: [
        {
          schwere: "blockiert",
          bereich: "identitaeten",
          text: '"9900357000004" ist eine BDEW-Codenummer (Strom), die Rolle lf-gas braucht eine DVGW-Codenummer (98…)',
          pfad: "rollen[1].mp_id",
          quelle: "BDEW-AWH Identifikatoren V1.2, Kap. 2.2 (S. 5)",
        },
      ],
      vollstaendig: false,
    };
    dienste(abgelehnt);
    show();
    const liste = await screen.findByRole("list", { name: "Abgelehnte Antworten" });
    expect(within(liste).getByText(/die Rolle lf-gas braucht eine DVGW-Codenummer/)).toBeTruthy();
    expect(within(liste).getByText("rollen[1].mp_id")).toBeTruthy();
    expect(
      within(liste).getByRole("button", {
        name: "Quelle: BDEW-AWH Identifikatoren V1.2, Kap. 2.2 (S. 5)",
      }),
    ).toBeTruthy();
  });

  it("works without the assistant: agentd down, the interview still builds the proposal", async () => {
    const { makod, agentd } = dienste(VORSCHLAG, "weg");
    show();
    fireEvent.change(await screen.findByRole("textbox", { name: "Aus einem Satz" }), {
      target: { value: "Wir sind Lieferant Strom und Gas." },
    });
    fireEvent.click(screen.getByRole("button", { name: /Vorbelegen/ }));
    await screen.findByText("Der Assistent ist nicht erreichbar");
    expect(agentd).toEqual(["POST /svc/agentd/api/v1/runs"]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Lieferant Strom" }));
    await waitFor(() => expect(JSON.stringify(makod.at(-1)?.args)).toContain('"rolle":"lf-strom"'));
    expect(screen.getByRole("tablist", { name: "Dateien" })).toBeTruthy();
  });

  it("offers each pre-filled answer and takes only the ones the reader confirms", async () => {
    const { makod } = dienste(VORSCHLAG, { angaben: VORBELEGT.angaben });
    show();
    await screen.findByRole("tablist", { name: "Dateien" });
    fireEvent.change(screen.getByRole("textbox", { name: "Aus einem Satz" }), {
      target: { value: "Lieferant Strom und Gas, Strom unter 9900357000004." },
    });
    fireEvent.click(screen.getByRole("button", { name: /Vorbelegen/ }));
    const angebote = await screen.findByRole("list", { name: "Vorgeschlagene Antworten" });
    expect(screen.getByText(/^AI-generated\./)).toBeTruthy();
    const zeilen = within(angebote).getAllByRole("listitem");
    expect(zeilen.map((z) => z.textContent)).toEqual([
      expect.stringContaining("Lieferant Strom unter 9900357000004"),
      expect.stringContaining("Lieferant Gas — MP-ID offen"),
      expect.stringContaining("Bilanzkreis Strom: 11XBK-STD------Y"),
      expect.stringContaining("Keine verbundenen Lieferanten"),
    ]);
    // Nothing is taken before the reader confirms it.
    expect(screen.queryByRole("textbox", { name: "MP-ID Lieferant Strom" })).toBeNull();
    const vorher = makod.length;
    fireEvent.click(within(zeilen[0] as HTMLElement).getByRole("button", { name: /Übernehmen/ }));
    const feld = await screen.findByRole("textbox", { name: "MP-ID Lieferant Strom" });
    expect((feld as HTMLInputElement).value).toBe("9900357000004");
    await waitFor(() => expect(makod.length).toBeGreaterThan(vorher));
    expect(JSON.stringify(makod.at(-1)?.args)).not.toContain("lf-gas");
    // The discarded one stays out.
    fireEvent.click(
      within(screen.getByRole("list", { name: "Vorgeschlagene Antworten" })).getAllByRole(
        "button",
        { name: "Verwerfen" },
      )[0] as HTMLElement,
    );
    expect(screen.queryByText(/Lieferant Gas — MP-ID offen/)).toBeNull();
  });
});
