import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import abgebrochenJsonl from "../../fixtures/run-abgebrochen.jsonl?raw";
import beantwortetJsonl from "../../fixtures/run-beantwortet.jsonl?raw";
import verborgenJsonl from "../../fixtures/run-verborgen.jsonl?raw";
import { HoldProvider } from "../app/Holds";
import { laeufeVergessen } from "../app/laeufe";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import type { Answer } from "../data/agent";
import { vergessen } from "../data/verweigert";
import {
  type AufgezeichneteQuelle,
  type Aufzeichnung,
  installieren,
  lesen,
  stromVon,
} from "../test-support/aufzeichnung";
import { Warum } from "./Warum";

// The trail against recorded runs: agentd's stream as `desk/fixtures/run-*.jsonl`
// records it, played into the Desk one event at a time — no agentd, no model.

const BEANTWORTET = lesen(beantwortetJsonl);
const ABGEBROCHEN = lesen(abgebrochenJsonl);
const VERBORGEN = lesen(verborgenJsonl);

const schritteVon = (lines: Aufzeichnung[]) => lines.filter((l) => l.event === "schritt");

/**
 * The answered fixture's lines as the trail shows them: the tool as an
 * operator calls it, agentd's verb, the object agentd names.
 */
const ZEILEN = [
  "Prozesse gelesen · Prozessschlüssel 51238696012",
  "Verlauf gelesen · Prozess 4711",
  "Freigabe gelesen · Freigabe q-4711",
  "offene Prozesse aufgelistet · MaLo 51238696012",
];
const antwortVon = (lines: Aufzeichnung[]) =>
  lines.find((l) => l.event === "antwort")?.data as Answer;

interface Agentd {
  stopps: string[];
  /** The run the next question starts. */
  run: string;
  /** What `GET …/antwort` answers, when the stream is not read. */
  antwort?: () => Response;
}

/** agentd's REST surface and the MCP reads the Prozess view makes. */
function agentd(a: Agentd) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
      if (url === "/svc/agentd/api/v1/runs" && init?.method === "POST") {
        return Response.json([{ agent_name: "desk-assistant", run_id: a.run }], { status: 202 });
      }
      const stopp = /^\/svc\/agentd\/api\/v1\/runs\/([^/]+)\/stopp$/.exec(url);
      if (stopp && init?.method === "POST") {
        a.stopps.push(stopp[1] ?? "");
        return new Response(null, { status: 202 });
      }
      if (url.endsWith("/antwort") && a.antwort) return a.antwort();
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      const m = JSON.parse(String(init?.body ?? "{}"));
      const result =
        m.method === "tools/call" && m.params.name === "get_process"
          ? {
              structuredContent: {
                business_key: "51238696012",
                processes: [
                  {
                    stream_id: "process/t/4711",
                    workflow_name: "gpke-supplier-change",
                    process_id: "4711",
                    deadlines: [],
                  },
                ],
              },
            }
          : m.method === "tools/call" && m.params.name === "get_process_history"
            ? { structuredContent: { process_id: "4711", workflow_name: "w", eintraege: [] } }
            : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

async function prozess() {
  const router = createDeskRouter(
    createMemoryHistory({ initialEntries: ["/prozess?key=51238696012"] }),
  );
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

/** Ask a question in Fragen and return the stream the Desk opened for it. */
async function fragen(run: string): Promise<AufgezeichneteQuelle> {
  const panel = await screen.findByRole("region", { name: "Fragen" });
  fireEvent.click(await within(panel).findByRole("button", { name: "Was ist hier offen?" }));
  await waitFor(() => expect(stromVon(run)).toBeTruthy());
  return stromVon(run) as AufgezeichneteQuelle;
}

const spur = () => screen.getByRole("list", { name: "Was der Assistent gelesen hat" });
const zeilen = () => within(spur()).getAllByRole("listitem");

beforeEach(() => {
  installieren(
    { beantwortet: BEANTWORTET, abgebrochen: ABGEBROCHEN, verborgen: VERBORGEN },
    vi.stubGlobal,
  );
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  vergessen();
  laeufeVergessen();
});

describe("the trail", () => {
  it("shows each step in journal order as it arrives, then the fixture's answer", async () => {
    agentd({ stopps: [], run: "beantwortet" });
    await prozess();
    const strom = await fragen("beantwortet");
    expect(schritteVon(BEANTWORTET)).toHaveLength(ZEILEN.length);

    act(() => strom.weiter());
    expect(zeilen()).toHaveLength(1);
    expect(zeilen()[0]?.textContent).toContain(ZEILEN[0]);
    // Live: the hairline runs, and Send has become Stop in its place.
    expect(screen.getByRole("status", { name: "Der Assistent liest" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Stoppen/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Fragen" })).toBeNull();

    act(() => strom.weiter(3));
    expect(zeilen().map((z) => z.textContent)).toEqual(
      ZEILEN.map((z, i) => expect.stringContaining(`${i + 1}${z}`)),
    );
    // Every step names its source through a chip.
    expect(within(spur()).getAllByRole("button", { name: /^Quelle: / })).toHaveLength(4);

    act(() => strom.alle());
    const a = antwortVon(BEANTWORTET);
    expect(screen.getByText(a.disclosure)).toBeTruthy();
    const saetze = within(screen.getByRole("list", { name: "Antwort" }))
      .getAllByRole("listitem")
      .map((li) => li.querySelector("p")?.textContent);
    expect(saetze).toEqual(a.saetze.map((s) => s.text));
    // Concluded: the stream is closed, Stop is Send again.
    expect(strom.readyState).toBe(strom.CLOSED);
    expect(screen.getByRole("button", { name: "Fragen" })).toBeTruthy();
    expect(screen.queryByRole("status", { name: "Der Assistent liest" })).toBeNull();
  });

  it("shows a step repeated after a reconnect once", async () => {
    agentd({ stopps: [], run: "beantwortet" });
    await prozess();
    const strom = await fragen("beantwortet");
    act(() => strom.weiter(2));
    act(() => {
      strom.fehler(false);
      // The browser reconnects with Last-Event-ID; a step at the seam comes twice.
      strom.senden(BEANTWORTET[1] as Aufzeichnung);
      strom.weiter();
    });
    expect(zeilen()).toHaveLength(3);
  });

  it("stops the run on Esc exactly once, and not while a popover owns Esc", async () => {
    const a: Agentd = { stopps: [], run: "beantwortet" };
    agentd(a);
    await prozess();
    const strom = await fragen("beantwortet");
    act(() => strom.weiter());

    // A source chip's popover is open: Esc closes it and stops nothing.
    fireEvent.click(within(spur()).getByRole("button", { name: /^Quelle: / }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(a.stopps).toEqual([]);

    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(a.stopps).toEqual(["beantwortet"]));
    expect(screen.getByRole("button", { name: /Wird gestoppt/ })).toBeTruthy();
  });

  it("marks the steps of a cancelled run „abgebrochen“ and keeps them", async () => {
    const a: Agentd = { stopps: [], run: "abgebrochen" };
    agentd(a);
    await prozess();
    const strom = await fragen("abgebrochen");
    act(() => strom.weiter(2));
    fireEvent.click(screen.getByRole("button", { name: /^Stoppen/ }));
    await waitFor(() => expect(a.stopps).toEqual(["abgebrochen"]));
    act(() => strom.alle());

    const shown = zeilen();
    expect(shown).toHaveLength(schritteVon(ABGEBROCHEN).length);
    for (const z of shown) expect(z.textContent).toContain("abgebrochen");
    // The call the stop refused stays, muted, as not answered.
    expect(shown[2]?.textContent).toContain("nicht beantwortet");
    expect(shown.filter((z) => z.textContent?.includes("nicht beantwortet"))).toHaveLength(1);
    expect(screen.getByText(/Gestoppt — der Assistent liest nicht weiter/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Fragen" })).toBeTruthy();
  });

  it("shows a step whose source is hidden as hidden, never drops it", async () => {
    agentd({ stopps: [], run: "verborgen" });
    await prozess();
    const strom = await fragen("verborgen");
    act(() => strom.weiter(3));
    expect(zeilen()).toHaveLength(3);
    const verborgen = zeilen()[1] as HTMLElement;
    expect(verborgen.textContent).toContain("Gelesen · Kunde k-1");
    expect(within(verborgen).getByText("Quelle für Sie verborgen")).toBeTruthy();
    expect(within(verborgen).queryByRole("button", { name: /^Quelle: / })).toBeNull();
  });
});

describe("the receipt", () => {
  it("folds the trail into the fixture's figures and opens it again", async () => {
    agentd({ stopps: [], run: "beantwortet" });
    await prozess();
    const strom = await fragen("beantwortet");
    act(() => strom.alle());
    // Four steps, each with a source; no duration — `um` is when agentd served a step.
    const quittung = screen.getByRole("button", { name: "4 Quellen gelesen" });
    expect(quittung.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("list", { name: "Was der Assistent gelesen hat" })).toBeNull();
    fireEvent.click(quittung);
    expect(quittung.getAttribute("aria-expanded")).toBe("true");
    expect(zeilen()).toHaveLength(4);
  });

  it("counts only the steps whose source the person may read", async () => {
    agentd({ stopps: [], run: "verborgen" });
    await prozess();
    const strom = await fragen("verborgen");
    act(() => strom.alle());
    expect(screen.getByRole("button", { name: "2 Quellen gelesen" })).toBeTruthy();
  });
});

describe("the evidence", () => {
  it("marks exactly the cited row on screen while a citation has focus", async () => {
    agentd({ stopps: [], run: "beantwortet" });
    await prozess();
    // The Verlauf of process 4711 is on screen beside the question.
    await screen.findByRole("heading", { name: "Verlauf" });
    const strom = await fragen("beantwortet");
    act(() => strom.alle());

    // The second sentence cites the Verlauf read — step 2 of the trail.
    const satz = screen.getByText(antwortVon(BEANTWORTET).saetze[1]?.text ?? "").closest("li");
    const beleg = within(satz as HTMLElement).getByRole("button", { name: "Beleg 2" });
    expect(document.querySelectorAll("[data-markiert]")).toHaveLength(0);
    act(() => beleg.focus());
    const markiert = document.querySelectorAll("[data-markiert]");
    expect(markiert).toHaveLength(1);
    expect(markiert[0]?.getAttribute("aria-labelledby")).toBe("verlauf-4711");
    act(() => beleg.blur());
    expect(document.querySelectorAll("[data-markiert]")).toHaveLength(0);

    // Enter opens the source: the trail unfolds on the cited step's chip.
    fireEvent.click(beleg);
    await waitFor(() =>
      expect(document.activeElement?.closest("li")?.textContent).toContain("Verlauf gelesen"),
    );
  });
});

describe("the pill", () => {
  it("follows a run out of Fragen with its latest step and leaves once the answer is read", async () => {
    agentd({ stopps: [], run: "beantwortet" });
    const router = await prozess();
    const strom = await fragen("beantwortet");
    act(() => strom.weiter(2));

    await act(() => router.navigate({ to: "/heute" }));
    await screen.findByRole("heading", { name: "Heute", level: 1 });
    const laeufe = () => screen.getByRole("list", { name: "Laufende Fragen" });
    expect(within(laeufe()).getByRole("button").textContent).toContain(
      "Verlauf gelesen · Prozess 4711",
    );
    // The run keeps streaming while its view is gone.
    act(() => strom.weiter());
    expect(within(laeufe()).getByRole("button").textContent).toContain(
      "Freigabe gelesen · Freigabe q-4711",
    );
    act(() => strom.alle());
    expect(within(laeufe()).getByRole("button").textContent).toContain("Antwort bereit");

    fireEvent.click(within(laeufe()).getByRole("button"));
    expect(await screen.findByText(antwortVon(BEANTWORTET).saetze[0]?.text ?? "none")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Laufende Fragen" })).toBeNull();
  });
});

describe("„Warum?“", () => {
  it("streams its trail, stops on Esc and falls back to the answer read without a stream", async () => {
    const a: Agentd = { stopps: [], run: "abgebrochen" };
    agentd(a);
    render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <RolleProvider>
          <HoldProvider>
            <Warum anker={{ malo: "51238696012" }} />
          </HoldProvider>
        </RolleProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(stromVon("abgebrochen")).toBeTruthy());
    const strom = stromVon("abgebrochen") as AufgezeichneteQuelle;
    act(() => strom.weiter());
    expect(zeilen()).toHaveLength(1);
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(a.stopps).toEqual(["abgebrochen"]));
    act(() => strom.alle());
    for (const z of zeilen()) expect(z.textContent).toContain("abgebrochen");
  });

  it("reads the answer when agentd refuses the stream, in agentd's words", async () => {
    agentd({
      stopps: [],
      run: "beantwortet",
      antwort: () =>
        Response.json(
          { error: "Forbidden", detail: "Diesen Lauf hat eine andere Person gestartet." },
          { status: 403 },
        ),
    });
    render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <RolleProvider>
          <HoldProvider>
            <Warum anker={{ malo: "51238696012" }} />
          </HoldProvider>
        </RolleProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(stromVon("beantwortet")).toBeTruthy());
    act(() => stromVon("beantwortet")?.fehler(true));
    expect(await screen.findByText(/Diesen Lauf hat eine andere Person gestartet/)).toBeTruthy();
  });
});
