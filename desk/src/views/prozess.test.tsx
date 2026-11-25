import { focusManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";
import { vergessen } from "../data/verweigert";

// The Prozess page reads top to bottom the same way every time: crumbs, the
// title with its state, one line of facts, the next Frist; the steps the
// history states; a decision bar only while a Freigabe is held for the
// process; the Verlauf with its Nachweise beside the details. Every figure on
// it is a service's.

// Route imports are lazy in production. This suite measures the process facts once the view is loaded.
beforeAll(async () => {
  await import("./Prozess");
});

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
});

const KEY = "51238696012";

const makodProcess = (deadlines: unknown[]) => ({
  business_key: KEY,
  processes: [
    {
      stream_id: "process/t/p-1",
      workflow_name: "gpke-supplier-change",
      process_id: "p-1",
      deadlines,
    },
  ],
});

const APERAK_OFFEN = {
  deadline_id: "d-a",
  label: "aperak-strom-45min-window",
  art: "aperak",
  due_at: "2031-01-01T09:45:00Z",
};
const ANTWORT_OFFEN = {
  deadline_id: "d-p",
  label: "gpke-antwortfrist",
  art: "prozess",
  due_at: "2031-01-02T10:00:00Z",
};

const projection = (fields: Record<string, unknown> = {}) => ({
  process_id: "p-1",
  pid: 55001,
  family: "gpke",
  workflow_name: "gpke-supplier-change",
  state: "running",
  malo_id: KEY,
  partner_mp_id: "9900357000004",
  mdm_role: null,
  deadline_at: "2031-01-02T10:00:00Z",
  deadline_source: "BK6-24-174 GPKE Teil 2",
  deadline_risk: "ok",
  started_at: "2031-01-01T09:00:00Z",
  last_event_at: "2031-01-01T09:00:00Z",
  erc_code: null,
  antwort_code: null,
  antwort_codeliste: null,
  bedeutung: null,
  initiator_is_affiliate: false,
  ...fields,
});

const eintrag = (
  sequence: number,
  recorded_at: string,
  event_type: string,
  richtung: string | null,
  pid: number | null,
) => ({
  sequence,
  recorded_at,
  event_type,
  richtung,
  pid,
  message_ref: richtung ? `REF-${sequence}` : null,
  antwortcode: null,
  codeliste: null,
  bedeutung: null,
});

const HISTORY = [
  eintrag(1, "2031-01-01T09:00:00Z", "SupplierChangeInitiated", "eingang", 55001),
  eintrag(2, "2031-01-01T09:01:00Z", "SupplierChangeValidationPassed", null, null),
];

const freigabe = {
  id: "q-1",
  process_id: "p-1",
  pid: 55001,
  malo_id: KEY,
  marktrolle: "NB",
  grund: "Anmeldung eines verbundenen Lieferanten (Antwortfrist 2031-01-02)",
  art: "gehalten",
  antwort: { code: "E15", ebd: "E_0623", bedeutung: "Zustimmung" },
  codes_freigeben: null,
  codes_ablehnen: {
    ebd: "E_0622",
    codes: [{ code: "A02", bedeutung: "nimmt nicht an der Marktkommunikation teil" }],
  },
  fakten_freigeben: [],
  fakten_ablehnen: [],
  kann_freigeben: true,
  kann_ablehnen: true,
  status: "Pending",
  expires_at: "2031-01-02T09:00:00Z",
  created_at: "2031-01-01T09:02:00Z",
  decided_by: null,
  decided_at: null,
};

function stub(opts: {
  deadlines: unknown[];
  history?: unknown[];
  /** The Marktrolle `get_process_history` names. */
  rolle?: string;
  freigaben?: unknown[];
  projection?: Record<string, unknown>;
  posts?: { url: string; body: unknown }[];
  /** processd lists `get_process` with another output schema than the snapshot. */
  driftProcessd?: boolean;
  /** agentd's proposal waiting on the Freigabe `q-1`. */
  wartend?: unknown;
}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.includes("/api/v1/queue/")) {
        opts.posts?.push({ url, body: JSON.parse(String(init?.body)) });
        return new Response(null, { status: 200 });
      }
      if (url.startsWith("/svc/agentd/api/v1/vorschlaege"))
        return Response.json({ freigabe: opts.wartend ? { "q-1": opts.wartend } : {} });
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      if (!init?.body) return Response.json({});
      const m = JSON.parse(String(init.body));
      let result: unknown = {};
      if (m.method === "tools/list" && opts.driftProcessd && url.startsWith("/svc/processd/"))
        result = {
          tools: [{ name: "get_process", inputSchema: {}, outputSchema: { type: "object" } }],
        };
      if (m.method === "tools/call") {
        const svc = url.split("/")[2];
        const tool = `${svc}/${m.params.name}`;
        if (tool === "makod/get_process")
          result = { structuredContent: makodProcess(opts.deadlines) };
        else if (tool === "makod/get_process_history")
          result = {
            structuredContent: {
              process_id: "p-1",
              workflow_name: "gpke-supplier-change",
              rolle: opts.rolle ?? null,
              eintraege: opts.history ?? HISTORY,
            },
          };
        else if (tool === "processd/get_process")
          result = { structuredContent: projection(opts.projection) };
        else if (tool === "processd/list_pending_approvals")
          result = {
            structuredContent: {
              freigaben: opts.freigaben ?? [],
              count: (opts.freigaben ?? []).length,
              saturated: false,
            },
          };
        else if (tool === "processd/rehearse_decision")
          result = {
            structuredContent: {
              id: "q-1",
              freigeben: true,
              urteil: "ohne_nachricht",
              command: null,
              marktrolle: null,
              payload: {},
              body_sha256: null,
              grund: null,
            },
          };
        else if (tool === "processd/list_counterparties")
          result = {
            structuredContent: {
              von: "2030-10-01",
              bis: "2031-01-01",
              gegenparteien: [
                {
                  mp_id: "9900357000004",
                  rolle: "LF",
                  gezaehlt: 3,
                  verspaetet: 0,
                  unbeantwortet: 0,
                  offen: 1,
                  ablehnungen: 0,
                },
              ],
              definitionen: [],
            },
          };
        else if (tool === "marktd/partner_namen")
          result = {
            structuredContent: {
              partner: [{ mp_id: "9900357000004", name: "Stadtwerke Musterstadt Netz GmbH" }],
            },
          };
      }
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

describe("the Prozess page", () => {
  it("heads the process with crumbs, its title and state, one fact line and the next Frist", async () => {
    stub({ deadlines: [ANTWORT_OFFEN, APERAK_OFFEN] });
    await open(`/prozess?key=${KEY}`);
    const h1 = await screen.findByRole("heading", { level: 1, name: /Lieferantenwechsel · Strom/ });
    const crumbs = screen.getByRole("navigation", { name: "Pfad" });
    expect(within(crumbs).getByRole("link", { name: "Prozesse" })).toBeTruthy();
    expect(within(crumbs).getByText(KEY)).toBeTruthy();
    // The state is processd's, through the catalogue.
    expect(await within(h1).findByText("läuft")).toBeTruthy();
    const fakten = screen.getByRole("list", { name: "Eckdaten" });
    expect(fakten.textContent).toContain(KEY);
    expect(fakten.textContent).toContain("PID 55001");
    // The Gegenpartei opens its record, in the one role processd lists for it.
    const gp = await within(fakten).findByRole("link", {
      name: "Stadtwerke Musterstadt Netz GmbH",
    });
    expect(gp.getAttribute("href")).toBe("/gegenpartei/9900357000004/LF");
    // The next Frist is the earliest pending one — the APERAK's here.
    const naechste = screen.getByRole("group", { name: "Nächste Frist" });
    expect(within(naechste).getByText("Eingangsbestätigung (APERAK)")).toBeTruthy();
  });

  it("names the role the process acts in and marks a message from the own house", async () => {
    stub({
      deadlines: [],
      rolle: "LF",
      history: [
        {
          ...eintrag(1, "2031-01-01T09:00:00Z", "InvoicReceived", "eingang", 31002),
          intern: true,
        },
      ],
    });
    await open(`/prozess?key=${KEY}`);
    expect(await screen.findByText("als Lieferant")).toBeTruthy();
    const chip = await screen.findByText("im Haus");
    expect(chip.getAttribute("title")).toContain("nicht über AS4");
  });

  it("steps through what the history states and what is still awaited", async () => {
    stub({ deadlines: [ANTWORT_OFFEN, APERAK_OFFEN] });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("list", { name: "Schritte" });
    await waitFor(() => expect(within(bar).getAllByRole("listitem")).toHaveLength(4));
    const schritte = within(bar)
      .getAllByRole("listitem")
      .map((li) => [li.querySelector("[data-text]")?.textContent, li.getAttribute("data-stand")]);
    expect(schritte).toEqual([
      ["Eingang · PID 55001", "erledigt"],
      ["APERAK", "jetzt"],
      ["Antwort", "offen"],
      ["abgeschlossen", "offen"],
    ]);
  });

  it("ticks each message in the Nachweise and leaves a missing APERAK open", async () => {
    stub({ deadlines: [ANTWORT_OFFEN, APERAK_OFFEN] });
    await open(`/prozess?key=${KEY}`);
    const kette = await screen.findByRole("list", { name: "Nachweise" });
    await waitFor(() => expect(within(kette).getAllByRole("listitem")).toHaveLength(3));
    const rows = within(kette).getAllByRole("listitem");
    expect(rows[0]?.getAttribute("data-belegt")).toBe("true");
    expect(rows[0]?.textContent).toContain("PID 55001");
    expect(rows[0]?.textContent).toContain("REF-1");
    expect(rows[1]?.getAttribute("data-belegt")).toBe("false");
    expect(rows[1]?.textContent).toContain("Eingangsbestätigung (APERAK)");
    expect(rows[1]?.textContent).toContain("steht aus");
  });

  it("marks an APERAK processd recorded as a refusal, not ticked", async () => {
    stub({ deadlines: [], projection: { erc_code: "Z29", state: "rejected" } });
    await open(`/prozess?key=${KEY}`);
    const kette = await screen.findByRole("list", { name: "Nachweise" });
    const refused = await within(kette).findByText(/Z29/);
    expect(refused.closest("li")?.getAttribute("data-belegt")).toBe("false");
    const bar = screen.getByRole("list", { name: "Schritte" });
    await waitFor(() =>
      expect(within(bar).getAllByRole("listitem").at(-1)?.getAttribute("data-stand")).toBe(
        "erledigt",
      ),
    );
    expect(
      within(bar).getAllByRole("listitem").at(-1)?.querySelector("[data-text]")?.textContent,
    ).toBe("abgelehnt");
  });

  it("renders no decision bar for a process with nothing to decide", async () => {
    stub({ deadlines: [ANTWORT_OFFEN] });
    await open(`/prozess?key=${KEY}`);
    await screen.findByRole("heading", { level: 1 });
    await screen.findByRole("list", { name: "Nachweise" });
    expect(screen.queryByRole("region", { name: "Wartet auf deine Entscheidung" })).toBeNull();
  });

  it("holds a 55001 for Freigabe in a decision bar with one primary action, and decides it from there", async () => {
    const posts: { url: string; body: unknown }[] = [];
    stub({ deadlines: [ANTWORT_OFFEN], freigaben: [freigabe], posts });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("region", { name: "Wartet auf deine Entscheidung" });
    expect(within(bar).getByText(/verbundenen Lieferanten/)).toBeTruthy();
    // processd is asked for this process's entry, not for a page to search.
    const gefragt = (fetch as unknown as { mock: { calls: [string, RequestInit?][] } }).mock.calls
      .filter(([, i]) => i?.body)
      .map(([, i]) => JSON.parse(String(i?.body)))
      .filter((m) => m.params?.name === "list_pending_approvals")
      .map((m) => m.params.arguments.process_id);
    expect(gefragt).toContain("p-1");
    // The rule behind the hold is the tree processd froze the answer in.
    expect(within(bar).getByRole("button", { name: "Quelle: E_0623" })).toBeTruthy();
    // One action: the decision opens under the bar.
    const knoepfe = within(bar)
      .getAllByRole("button")
      .filter((b) => !b.getAttribute("aria-label")?.startsWith("Quelle"));
    expect(knoepfe.map((b) => b.textContent)).toEqual(["Entscheiden"]);
    fireEvent.click(knoepfe[0] as HTMLElement);
    fireEvent.click(await screen.findByRole("radio", { name: "Freigeben" }));
    fireEvent.click(screen.getByRole("button", { name: "Freigeben" }));
    expect(posts).toEqual([]);
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() =>
      expect(posts.map((p) => p.url)).toEqual(["/svc/processd/api/v1/queue/q-1/approve"]),
    );
  });

  const wartend = (urteil: string) => ({
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
      stufen:
        urteil === "abgelehnt"
          ? [
              {
                stufe: "entscheidung",
                urteil,
                code: "code_outside_tree",
                regel: "E_0622",
                segment: null,
                text: "code A99 is not in E_0622",
              },
            ]
          : [],
      grund: urteil === "nicht_geprueft" ? "unreachable" : null,
      body_sha256: null,
      geprueft_um: "2031-01-01T09:03:00Z",
    },
    schritte: [],
    hilfreich: null,
  });

  it("an_unchecked_agent_proposal_offers_bearbeiten_only", async () => {
    stub({ deadlines: [ANTWORT_OFFEN], freigaben: [freigabe], wartend: wartend("nicht_geprueft") });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("region", { name: "Wartet auf deine Entscheidung" });
    await within(bar).findByText(/Nicht geprüft \(unreachable\)/);
    const knoepfe = within(bar)
      .getAllByRole("button")
      .filter((b) => !b.getAttribute("aria-label")?.startsWith("Quelle"));
    expect(knoepfe.map((b) => b.textContent)).toEqual(["Bearbeiten"]);
    // Bearbeiten opens the decision with nothing taken in from the proposal.
    fireEvent.click(knoepfe[0] as HTMLElement);
    const radio = (await screen.findByRole("radio", { name: "Freigeben" })) as HTMLInputElement;
    expect(radio.checked).toBe(false);
  });

  it("offers a proposal the gate refused as Bearbeiten, with the rule it broke", async () => {
    stub({ deadlines: [ANTWORT_OFFEN], freigaben: [freigabe], wartend: wartend("abgelehnt") });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("region", { name: "Wartet auf deine Entscheidung" });
    expect(await within(bar).findByText(/E_0622/)).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "Bearbeiten" })).toBeTruthy();
    expect(within(bar).queryByRole("button", { name: "Übernehmen" })).toBeNull();
  });

  it("offers a proposal that passed the gate to take in, unsent", async () => {
    stub({ deadlines: [ANTWORT_OFFEN], freigaben: [freigabe], wartend: wartend("bestanden") });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("region", { name: "Wartet auf deine Entscheidung" });
    fireEvent.click(await within(bar).findByRole("button", { name: "Übernehmen" }));
    const radio = (await screen.findByRole("radio", { name: "Freigeben" })) as HTMLInputElement;
    await waitFor(() => expect(radio.checked).toBe(true));
  });

  it("a_drifted_processd_withholds_the_decide_action", async () => {
    stub({ deadlines: [ANTWORT_OFFEN], freigaben: [freigabe], driftProcessd: true });
    await open(`/prozess?key=${KEY}`);
    const bar = await screen.findByRole("region", { name: "Wartet auf deine Entscheidung" });
    // The banner names the drift, and the bar offers no decision meanwhile.
    expect(await screen.findByRole("alert")).toBeTruthy();
    await waitFor(() =>
      expect(within(bar).queryByRole("button", { name: "Entscheiden" })).toBeNull(),
    );
    expect(within(bar).getByText(/Entscheiden geht erst wieder/)).toBeTruthy();
    // The banner names the service, as an operator knows it (SR-043).
    expect((await screen.findByRole("alert")).textContent).toContain("Prozessüberwachung");
  });

  it("labels each message with who sent it, the agent's colour only where a proposal was involved", async () => {
    stub({
      deadlines: [],
      history: [
        {
          ...eintrag(1, "2031-01-01T09:00:00Z", "SupplierChangeInitiated", "eingang", 55001),
          akteur: "system",
        },
        {
          ...eintrag(2, "2031-01-01T10:00:00Z", "AntwortGesendet", "ausgang", 55002),
          akteur: "freigabe-geaendert",
        },
      ],
    });
    await open(`/prozess?key=${KEY}`);
    const label = await screen.findByText("freigegeben, Vorschlag geändert");
    expect(label.className).toContain("text-ai");
    const system = screen.getByText("automatisch");
    expect(system.className).not.toContain("text-ai");
  });

  it("shows the process as of an earlier event with the Stand slider", async () => {
    stub({
      deadlines: [ANTWORT_OFFEN],
      history: [
        ...HISTORY,
        eintrag(3, "2031-01-01T10:00:00Z", "SupplierChangeAntwortGesendet", "ausgang", 55002),
      ],
    });
    await open(`/prozess?key=${KEY}`);
    expect(await screen.findByText("PID 55002", { selector: "span" })).toBeTruthy();
    const regler = screen.getByRole("slider", { name: "Stand" });
    // Back to the first event: the answer was not sent yet, and the Frist
    // pending now is not shown as pending then.
    fireEvent.change(regler, { target: { value: "1" } });
    await waitFor(() => expect(screen.queryByText("PID 55002", { selector: "span" })).toBeNull());
    expect(screen.getByText(/Stand zu 01\.01\.2031, 10:00/)).toBeTruthy();
    const kette = screen.getByRole("list", { name: "Nachweise" });
    expect(within(kette).getAllByRole("listitem")).toHaveLength(1);
  });

  it("renders a counterparty's text with a script tag as text, never as markup", async () => {
    const boese = "<script>window.__boese = 1</script><b>fett</b>";
    stub({
      deadlines: [],
      history: [
        {
          ...eintrag(1, "2031-01-01T09:00:00Z", "AnswerReceived", "eingang", 55002),
          antwortcode: "A02",
          codeliste: "E_0622",
          bedeutung: boese,
        },
      ],
    });
    await open(`/prozess?key=${KEY}`);
    expect(await screen.findByText(boese)).toBeTruthy();
    expect(document.querySelector("main script, main b")).toBeNull();
    expect((window as unknown as { __boese?: number }).__boese).toBeUndefined();
  });

  it("switches between several processes under one key, the earliest Frist first", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url.startsWith("/svc/agentd/")) return Response.json([]);
        if (!init?.body) return Response.json({});
        const m = JSON.parse(String(init.body));
        const p = (id: string, due: string) => ({
          stream_id: `process/t/${id}`,
          workflow_name: `wf-${id}`,
          process_id: id,
          deadlines: [{ deadline_id: `d-${id}`, label: "x", art: "prozess", due_at: due }],
        });
        const result =
          m.method === "tools/call" &&
          m.params.name === "get_process" &&
          m.params.arguments.business_key
            ? {
                structuredContent: {
                  business_key: KEY,
                  processes: [p("late", "2031-12-01T10:00:00Z"), p("soon", "2031-11-05T10:00:00Z")],
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open(`/prozess?key=${KEY}`);
    const nav = await screen.findByRole("navigation", { name: `2 Prozesse unter ${KEY}` });
    const links = within(nav).getAllByRole("link");
    expect(
      links.map(
        (l) => l.textContent?.startsWith("wf-soon") || l.textContent?.startsWith("wf-late"),
      ),
    ).toEqual([true, true]);
    expect(links[0]?.textContent).toContain("wf-soon");
    expect(links[0]?.getAttribute("aria-current")).toBe("page");
    expect(await screen.findByRole("heading", { level: 1, name: /wf-soon/ })).toBeTruthy();
    fireEvent.click(links[1] as HTMLElement);
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("wf-late"),
    );
  });
});

describe("the Nachweise (SR-045)", () => {
  it("calls a sent message with no Zustellquittung recorded, never delivered", async () => {
    stub({
      deadlines: [],
      history: [
        {
          ...eintrag(1, "2031-01-01T09:00:00Z", "AntwortGesendet", "ausgang", 55002),
          akteur: "system",
        },
      ],
    });
    await open(`/prozess?key=${KEY}`);
    const kette = await screen.findByRole("list", { name: "Nachweise" });
    const zeile = (await within(kette).findAllByRole("listitem"))[0] as HTMLElement;
    expect(zeile.textContent).toContain("Ausgang · PID 55002");
    expect(zeile.textContent).toContain("aufgezeichnet");
    expect(zeile.textContent).not.toMatch(/zugestellt/);
  });
});

describe("drift on focus (SR-043)", () => {
  it("asks each service's tools again when the window regains focus", async () => {
    stub({ deadlines: [] });
    await open(`/prozess?key=${KEY}`);
    await screen.findByRole("heading", { level: 1, name: /Lieferantenwechsel/ });
    const lists = () =>
      (fetch as unknown as { mock: { calls: [string, RequestInit?][] } }).mock.calls.filter(
        ([u, i]) => u.startsWith("/svc/processd/mcp") && String(i?.body).includes('"tools/list"'),
      ).length;
    await waitFor(() => expect(lists()).toBeGreaterThan(0));
    const vorher = lists();
    // A minute later the check is stale; focus brings it back.
    vi.setSystemTime(Date.now() + 61_000);
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    await waitFor(() => expect(lists()).toBeGreaterThan(vorher));
    vi.useRealTimers();
  });
});
