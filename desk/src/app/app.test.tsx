import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SHORTCUTS } from "./keys";
import { RolleProvider } from "./rollen";
import { createDeskRouter } from "./router";

// Every screen mounts in the real shell without throwing — the check a
// typecheck cannot make about React Aria's runtime contracts.

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

// Each test renders its own app on its own router and unmounts it after, so
// no test sees another's calls.
afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

describe("the Desk", () => {
  it("opens on Prozesse with the Rolle badge in the toolbar", async () => {
    await open("/");
    expect(await screen.findByRole("heading", { name: "Prozesse", level: 1 })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Rolle: Netz/ })).toBeTruthy();
    expect(screen.getByText("Welcher Prozess?")).toBeTruthy();
  });

  it("shows makod's refusal for an unknown key", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        const answer =
          m.method === "tools/call"
            ? { isError: true, content: [{ type: "text", text: "process_not_found: none" }] }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: answer }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/prozess?key=51238696012");
    expect(await screen.findByText("Kein Prozess für „51238696012“")).toBeTruthy();
    const sent = (fetch as unknown as { mock: { calls: [string, RequestInit][] } }).mock.calls
      .map(([, init]) => JSON.parse(String(init.body)))
      .find((m) => m.method === "tools/call" && m.params.name === "get_process");
    // An identifier leaves the URL as the string it was — never a number.
    expect(sent?.params.arguments.business_key).toBe("51238696012");
    expect(screen.getByText("process_not_found: none")).toBeTruthy();
  });

  it("lists every process under a key, the earliest Frist first", async () => {
    const process = (id: string, due: string) => ({
      stream_id: `process/t/${id}`,
      workflow_name: `wf-${id}`,
      process_id: id,
      deadlines: [{ deadline_id: `d-${id}`, label: "response", art: "prozess", due_at: due }],
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        const answer =
          m.method === "tools/call"
            ? {
                structuredContent: {
                  business_key: "51238696012",
                  processes: [
                    process("late", "2026-12-01T10:00:00Z"),
                    process("soon", "2026-11-05T10:00:00Z"),
                  ],
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: answer }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/prozess?key=51238696012");
    expect(await screen.findByText("2 Prozesse")).toBeTruthy();
    const titles = screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent);
    expect(titles.filter((t) => t?.startsWith("wf-"))).toEqual(["wf-soon", "wf-late"]);
  });

  it("shows a customer with each Lieferstelle in its own state", async () => {
    const calls: string[] = [];
    const offen = (process_id: string, deadline_at: string) => ({
      process_id,
      pid: 55001,
      family: "gpke",
      state: "running",
      malo_id: "51238696012",
      partner_mp_id: null,
      started_at: "2030-12-01T00:00:00Z",
      deadline_at,
      deadline_source: null,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        if (m.method === "tools/call") calls.push(`${url.split("/")[2]}/${m.params.name}`);
        if (url.includes("/svc/processd/") && m.method === "tools/call") {
          // processd answers the earliest Frist first; the second process on
          // the same MaLo is later and must not be the one shown.
          const result = {
            structuredContent: {
              prozesse: [offen("p1", "2031-01-01T10:00:00Z"), offen("p2", "2031-06-01T10:00:00Z")],
              weiter: null,
            },
          };
          return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
            headers: { "content-type": "application/json", "mcp-session-id": "s" },
          });
        }
        const stelle = (malo_id: string, status: string, lieferende: string | null) => ({
          malo_id,
          sparte: "STROM",
          vertrags_nr: `VV-${malo_id}`,
          standort: null,
          lieferbeginn: "2026-12-01",
          lieferende,
          status,
        });
        const answer =
          m.method === "tools/call"
            ? {
                structuredContent: {
                  id: "k-1",
                  kunden_nr: "K-10042",
                  kundentyp: "B2C",
                  haushaltskunde: true,
                  name: "Erika Mustermann",
                  lieferstellen: [
                    stelle("51238696012", "ANGEMELDET", null),
                    stelle("51238696782", "BEENDET", "2026-01-31"),
                  ],
                  lieferstellen_gesamt: 250,
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: answer }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/kunde/k-1");
    expect(await screen.findByRole("heading", { name: "Erika Mustermann", level: 1 })).toBeTruthy();
    const anmeldung = screen.getAllByRole("link", { name: /51238696012: Anmeldung läuft/ })[0];
    expect(anmeldung?.getAttribute("href")).toBe("/prozess?key=51238696012");
    expect(screen.getAllByText("beendet zum 31.01.2026").length).toBeGreaterThan(0);
    // One Frist, the earliest, on the Lieferstelle with open processes.
    const frist = await screen.findAllByText(/^in \d+ Tagen$/);
    expect(frist).toHaveLength(1);
    expect(frist[0]?.closest("time")?.getAttribute("datetime")).toBe("2031-01-01T10:00:00Z");
    // The total is vertragd's; the Fristen are one processd call, never one
    // makod call per Lieferstelle.
    expect(screen.getByText(/Die neuesten 2 von 250/)).toBeTruthy();
    expect(calls.filter((c) => c.endsWith("/list_open_processes"))).toHaveLength(1);
    expect(calls.filter((c) => c.includes("makod"))).toHaveLength(0);
  });

  it("opens Heute when the person puts on the Lieferant", async () => {
    await open("/prozess");
    await screen.findByRole("heading", { name: "Prozesse", level: 1 });
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "2", ctrlKey: true }));
    });
    expect(await screen.findByRole("heading", { name: "Heute", level: 1 })).toBeTruthy();
  });

  it("keeps processd's Frist order and names a page with one vertragd call", async () => {
    const row = (process_id: string, malo_id: string, deadline_at: string | null) => ({
      process_id,
      pid: 55001,
      family: "gpke",
      state: "running",
      malo_id,
      partner_mp_id: null,
      started_at: "2031-01-01T00:00:00Z",
      deadline_at,
      deadline_source: null,
    });
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        let result: unknown = {};
        if (m.method === "tools/call") {
          calls.push(m.params.name);
          if (m.params.name === "list_open_processes") {
            result = {
              structuredContent: {
                prozesse: [
                  row("p1", "41203768152", "2031-01-01T10:00:00Z"),
                  row("p2", "51238696012", "2031-03-01T10:00:00Z"),
                  row("p3", "30871225644", null),
                ],
                weiter: "2031-03-01T10:00:00Z~p2",
              },
            };
          } else if (m.params.name === "kunden_zu_malos") {
            const k = (malo_id: string, name: string) => ({
              malo_id,
              kunden_id: name,
              name,
              kunden_nr: null,
              sparte: "STROM",
              status: "ANGEMELDET",
            });
            result = {
              structuredContent: {
                kunden: [
                  k("30871225644", "Ohne Frist"),
                  k("51238696012", "Spät"),
                  k("41203768152", "Früh"),
                ],
              },
            };
          } else {
            result = { structuredContent: { anmeldungen: [], gesamt: 0 } };
          }
        }
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/heute");
    await screen.findByText("Früh");
    const rows = screen.getAllByRole("row").map((r) => r.textContent ?? "");
    expect(rows).toHaveLength(3);
    expect(rows[0]).toContain("Früh");
    expect(rows[1]).toContain("Spät");
    expect(rows[2]).toContain("keine Frist");
    // A page of rows costs one read for the order and one for the names —
    // never a call per row, however many rows a tenant holds.
    expect(calls.filter((c) => c === "list_open_processes")).toHaveLength(1);
    expect(calls.filter((c) => c === "kunden_zu_malos")).toHaveLength(1);
    expect(calls.filter((c) => c === "get_process")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Nächste 50" })).toBeTruthy();
  });

  it("states the Lage in the services' own counts, with no model in it", async () => {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body ?? "{}"));
        calls.push(`${url.split("/")[2]}/${m.params?.name ?? m.method ?? ""}`);
        let result: unknown = {};
        if (m.params?.name === "count_open_processes") {
          result = {
            structuredContent: {
              offen: 1234,
              heute_faellig: 3,
              heute_frueheste: "2031-01-01T10:00:00Z",
            },
          };
        } else if (m.params?.name === "list_overdue_processes") {
          result = { structuredContent: { overdue: [], count: 2, saturated: false, limit: 500 } };
        } else if (m.params?.name === "list_anmeldungen") {
          result = { structuredContent: { anmeldungen: [], gesamt: 1 } };
        } else if (m.params?.name === "list_open_processes") {
          result = { structuredContent: { prozesse: [], weiter: null } };
        }
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/heute");
    const lage = within(await screen.findByRole("region", { name: "Lage" }));
    // Each figure is the one its service returned — the total is processd's
    // count, not the length of a page — and a Frist's time is Berlin's.
    expect(await lage.findByText(/3 Fristen laufen heute ab/)).toBeTruthy();
    expect(lage.getByText(/die früheste um 11:00/)).toBeTruthy();
    expect(lage.getByText(/1\.234 Anmeldungen warten auf den Netzbetreiber/)).toBeTruthy();
    expect(lage.getByText(/2 Fristen sind überschritten/)).toBeTruthy();
    expect(lage.getByText(/1 Anmeldung ist noch nicht übermittelt/)).toBeTruthy();
    expect(calls.some((c) => c.includes("runs"))).toBe(false);
  });

  it("shows each Überblick number as its service returned it, linking to its list", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        let result: unknown = {};
        if (m.method === "tools/call" && url.includes("/svc/processd/")) {
          result = {
            structuredContent: {
              overdue: [
                {
                  process_id: "p-1",
                  pid: 55001,
                  family: "gpke",
                  state: "running",
                  malo_id: "51238696012",
                  partner_mp_id: null,
                  started_at: "2026-10-01T08:00:00Z",
                  deadline_at: "2026-10-02T09:00:00Z",
                  deadline_source: "BK6-24-174 GPKE Teil 2",
                },
              ],
              count: 500,
              saturated: true,
              limit: 500,
            },
          };
        } else if (m.method === "tools/call") {
          result = { structuredContent: { anmeldungen: [], gesamt: 1284 } };
        }
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/ueberblick");
    // A capped answer is shown as the floor it is; the total is vertragd's,
    // not the length of the page it sent.
    const fristen = await screen.findByRole("link", { name: "Fristen überschritten: 500+" });
    expect(fristen.getAttribute("href")).toBe("/fristen");
    const offen = await screen.findByRole("link", { name: "Anmeldungen offen: 1.284" });
    expect(offen.getAttribute("href")).toBe("/heute");
    localStorage.removeItem("mako-desk.rolle");
  });

  it("lists each overdue process with its Fundstelle and its process link", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        const result =
          m.method === "tools/call"
            ? {
                structuredContent: {
                  overdue: [
                    {
                      process_id: "p-1",
                      pid: 55001,
                      family: "gpke",
                      state: "running",
                      malo_id: "51238696012",
                      partner_mp_id: null,
                      started_at: "2026-10-01T08:00:00Z",
                      deadline_at: "2026-10-02T09:00:00Z",
                      deadline_source: "BK6-24-174 GPKE Teil 2",
                    },
                  ],
                  count: 1,
                  saturated: false,
                  limit: 500,
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/fristen");
    const malo = await screen.findByRole("link", { name: "51238696012" });
    expect(malo.getAttribute("href")).toBe("/prozess?key=51238696012");
    expect(screen.getByRole("button", { name: "Quelle: BK6-24-174 GPKE Teil 2" })).toBeTruthy();
    expect(screen.getByText(/^abgelaufen vor /)).toBeTruthy();
  });

  it("threads a process's messages by direction, with its pending Frist at the end", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        let result: unknown = {};
        if (m.method === "tools/call" && m.params.name === "get_process") {
          result = {
            structuredContent: {
              business_key: "51238696012",
              processes: [
                {
                  stream_id: "s",
                  workflow_name: "gpke-supplier-change",
                  process_id: "p-1",
                  deadlines: [
                    {
                      deadline_id: "d",
                      label: "gpke-antwortfrist",
                      art: "prozess",
                      due_at: "2031-01-01T10:00:00Z",
                    },
                  ],
                },
              ],
            },
          };
        } else if (m.method === "tools/call" && m.params.name === "get_process_history") {
          const e = (
            sequence: number,
            event_type: string,
            richtung: string | null,
            pid: number | null,
          ) => ({
            sequence,
            recorded_at: "2030-12-01T09:00:00Z",
            event_type,
            richtung,
            pid,
            message_ref: richtung === "eingang" ? "UTILMD-1" : null,
          });
          result = {
            structuredContent: {
              process_id: "p-1",
              workflow_name: "gpke-supplier-change",
              eintraege: [
                e(1, "SupplierChangeInitiated", "eingang", 55001),
                e(2, "SupplierChangeValidationPassed", null, null),
                e(3, "SupplierChangeActivated", null, null),
                e(4, "SupplierChangeAntwortGesendet", "ausgang", 55002),
              ],
            },
          };
        }
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/prozess?key=51238696012");
    const eingang = await screen.findByText("PID 55001");
    const ausgang = screen.getByText("PID 55002");
    // Direction is the side: Eingang in the left column, Ausgang in the right.
    expect(eingang.closest("li")?.firstElementChild?.textContent).toContain("Eingang");
    expect(ausgang.closest("li")?.lastElementChild?.textContent).toContain("Ausgang");
    // Steps between messages fold into one quiet row; the identifier is
    // there on request, unparsed. The Frist closes the thread.
    expect(screen.getByText("2 interne Schritte")).toBeTruthy();
    expect(screen.getByText("SupplierChangeValidationPassed")).toBeTruthy();
    const rows = [...(eingang.closest("ol")?.children ?? [])].map((li) => li.textContent ?? "");
    expect(rows).toHaveLength(4);
    // The Frist says what it is for; its label stays the tooltip.
    expect(rows[3]).toContain("Frist des Prozesses");
    expect(screen.getByText("Frist des Prozesses").getAttribute("title")).toBe("gpke-antwortfrist");
    expect(screen.getByText("UTILMD-1")).toBeTruthy();
  });

  it("asks the assistant about the MaLo and marks each sentence by its check", async () => {
    const asked: unknown[] = [];
    const verdicts: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/svc/agentd/api/v1/agents/catalog") {
          return Response.json({
            faehigkeiten: [
              {
                name: "desk-assistant",
                titel: "Assistent im Desk",
                zweck: null,
                version: "1.0.0",
                digest: null,
                aktiv: true,
                ausloeser: [],
                modell: "anthropic/claude-sonnet-5",
                liest: [],
                nur_lesend: true,
                mit_freigabe: 0,
              },
            ],
          });
        }
        if (url === "/svc/agentd/api/v1/runs/r-1/bewertung") {
          verdicts.push(JSON.parse(String(init?.body)));
          return new Response(null, { status: 204 });
        }
        if (url.startsWith("/svc/agentd/api/v1/runs/")) {
          return Response.json({
            run: "r-1",
            status: "succeeded",
            disclosure: "AI-generated.",
            hilfreich: null,
            schritte: [
              { quelle: "makod/get_process", fertig: true },
              { quelle: "processd/list_open_processes", fertig: true },
            ],
            saetze: [
              {
                text: "Für 51238696012 endet die Frist am 05.11.2026 um 11:00 Uhr.",
                quelle: "makod/get_process",
                belegt: true,
              },
              { text: "Das sind noch 3 Tage.", quelle: "makod/get_process", belegt: false },
            ],
          });
        }
        if (url === "/svc/agentd/api/v1/runs") {
          asked.push(JSON.parse(String(init?.body)));
          return Response.json(
            [{ agent_name: "desk-assistant", run_id: "r-1", fresh: true, retryable: false }],
            {
              status: 202,
            },
          );
        }
        const m = JSON.parse(String(init?.body));
        const result =
          m.method === "tools/call" && m.params.name === "get_process"
            ? {
                structuredContent: {
                  business_key: "51238696012",
                  processes: [
                    {
                      stream_id: "s",
                      workflow_name: "gpke-supplier-change",
                      process_id: "p",
                      deadlines: [],
                    },
                  ],
                },
              }
            : m.method === "tools/call"
              ? { structuredContent: { process_id: "p", workflow_name: "w", eintraege: [] } }
              : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/prozess?key=51238696012");
    // The model that answers, named before anything is asked.
    expect(
      await screen.findByText("Antwortet mit claude-sonnet-5 · anthropic · liest nur, handelt nie"),
    ).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Was ist hier offen?" }));
    expect(await screen.findByText("AI-generated.")).toBeTruthy();
    expect(asked).toEqual([
      {
        agent: "desk-assistant",
        event_type: "manual",
        input: { frage: "Was ist hier offen?", malo: "51238696012" },
      },
    ]);
    // The source is named for an operator, and only the unchecked sentence is marked.
    expect(screen.getAllByText("Marktkommunikation · Prozesse")).toHaveLength(2);
    expect(screen.getAllByText("nicht belegt")).toHaveLength(1);
    // Every read the run made is named, including one no sentence cites.
    expect(
      screen.getByText(/^Gelesen: Marktkommunikation · Prozesse, Prozessüberwachung/),
    ).toBeTruthy();
    expect(screen.getByText("Das sind noch 3 Tage.").closest("li")?.textContent).toContain(
      "nicht belegt",
    );

    // „Nein" is recorded at once; the reason, if given, replaces it with words.
    fireEvent.click(screen.getByRole("button", { name: "Nein" }));
    await waitFor(() => expect(verdicts).toEqual([{ hilfreich: false }]));
    const warum = screen.getByRole("textbox", { name: "Was fehlte?" });
    fireEvent.change(warum, { target: { value: "Die Frist des LF fehlt." } });
    fireEvent.submit(warum.closest("form") as HTMLFormElement);
    await waitFor(() =>
      expect(verdicts).toEqual([
        { hilfreich: false },
        { hilfreich: false, begruendung: "Die Frist des LF fehlt." },
      ]),
    );
    expect(await screen.findByText("Danke — das bleibt beim Lauf.")).toBeTruthy();
  });

  it("offers no Fragen without agentd, and the process still shows", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url.startsWith("/svc/agentd/")) throw new TypeError("connection refused");
        const m = JSON.parse(String(init?.body));
        const result =
          m.method === "tools/call" && m.params.name === "get_process"
            ? {
                structuredContent: {
                  business_key: "51238696012",
                  processes: [
                    {
                      stream_id: "s",
                      workflow_name: "gpke-supplier-change",
                      process_id: "p",
                      deadlines: [],
                    },
                  ],
                },
              }
            : m.method === "tools/call"
              ? { structuredContent: { process_id: "p", workflow_name: "w", eintraege: [] } }
              : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/prozess?key=51238696012");
    expect(await screen.findByText("Lieferantenwechsel · Strom")).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole("region", { name: "Fragen" })).toBeNull());
    expect(screen.queryByText(/nicht erreichbar/)).toBeNull();
  });

  /** A Freigabe as processd serves it, and a fetch answering the Desk's reads. */
  function freigabeFetch(opts: {
    posts: { url: string; body: unknown }[];
    vorschlag?: Record<string, unknown>;
  }) {
    const freigabe = {
      id: "q-1",
      process_id: "p-1",
      pid: 55001,
      malo_id: "51238696012",
      marktrolle: "NB",
      grund: "Anmeldung eines verbundenen Lieferanten (Antwortfrist 2031-01-02)",
      art: "eskaliert",
      antwort: null,
      codes_freigeben: {
        ebd: "E_0623",
        codes: [{ code: "E15", bedeutung: "Zustimmung" }],
      },
      codes_ablehnen: {
        ebd: "E_0622",
        codes: [
          { code: "A01", bedeutung: "Marktlokation nicht gefunden" },
          { code: "A02", bedeutung: "nimmt nicht an der Marktkommunikation teil" },
        ],
      },
      kann_freigeben: true,
      kann_ablehnen: true,
      status: "Pending",
      expires_at: "2031-01-02T09:00:00Z",
      created_at: "2031-01-01T08:00:00Z",
      decided_by: null,
      decided_at: null,
    };
    return vi.fn(async (url: string, init?: RequestInit) => {
      // The writes under test: a decision, and what became of a proposal.
      if (url.includes("/api/v1/queue/") || url.endsWith("/bewertung")) {
        opts.posts.push({ url, body: JSON.parse(String(init?.body)) });
        return new Response(null, { status: url.endsWith("/bewertung") ? 204 : 200 });
      }
      if (url === "/svc/agentd/api/v1/agents/catalog") {
        return Response.json({ faehigkeiten: [] });
      }
      if (url === "/svc/agentd/api/v1/runs") {
        return Response.json([{ agent_name: "desk-assistant", run_id: "r-9" }], { status: 202 });
      }
      if (url.startsWith("/svc/agentd/api/v1/runs/r-9/antwort")) {
        return Response.json({
          run: "r-9",
          status: "succeeded",
          disclosure: "AI-generated.",
          saetze: [{ text: "Die MaLo nimmt nicht teil.", quelle: null, belegt: false }],
          vorschlag: opts.vorschlag ?? null,
          schritte: [],
          hilfreich: null,
        });
      }
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      const m = JSON.parse(String(init?.body));
      const result =
        m.method === "tools/call" && m.params.name === "list_pending_approvals"
          ? { structuredContent: { freigaben: [freigabe], count: 1, saturated: false } }
          : m.method === "tools/call"
            ? { structuredContent: { process_id: "p-1", workflow_name: "w", eintraege: [] } }
            : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    });
  }

  it("decides a Freigabe with a code its tree defines, sent only after the confirm", async () => {
    const posts: { url: string; body: unknown }[] = [];
    vi.stubGlobal("fetch", freigabeFetch({ posts }));
    await open("/freigaben");
    expect(await screen.findByRole("heading", { name: /verbundenen Lieferanten/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: "Ablehnen" }));
    // The codes are the tree's own, each with its wording.
    expect(screen.getByText("E_0622")).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    expect(posts).toEqual([]);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A02", antwort_tree: "E_0622" },
        },
      ]),
    );
    expect(await screen.findByText("Entschieden.")).toBeTruthy();
  });

  it("takes the assistant's proposal into the form and sends nothing until the person does", async () => {
    const posts: { url: string; body: unknown }[] = [];
    vi.stubGlobal(
      "fetch",
      freigabeFetch({
        posts,
        vorschlag: {
          aktion: "ablehnen",
          antwortcode: "A02",
          begruendung: "Die MaLo nimmt nicht an der Marktkommunikation teil.",
          quelle: "processd/get_queue_entry",
          belegt: true,
        },
      }),
    );
    await open("/freigaben?id=q-1");
    fireEvent.click(await screen.findByRole("button", { name: "Was schlägst du vor?" }));
    fireEvent.click(await screen.findByRole("button", { name: "In die Entscheidung übernehmen" }));
    // Prefilled, unsent.
    expect((screen.getByRole("radio", { name: "Ablehnen" }) as HTMLInputElement).checked).toBe(
      true,
    );
    expect((screen.getByRole("radio", { name: /A02/ }) as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText(/Vom Assistenten vorgeschlagen/)).toBeTruthy();
    expect(posts).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    // The same request the manual form sends, then what became of the proposal.
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A02", antwort_tree: "E_0622" },
        },
        { url: "/svc/agentd/api/v1/runs/r-9/bewertung", body: { vorschlag: "sent" } },
      ]),
    );
  });

  it("shows each skill's Beipackzettel from the catalog, the inactive ones on request", async () => {
    const skill = (name: string, titel: string, aktiv: boolean, modell: string | null) => ({
      name,
      titel,
      zweck: `${titel} — Zweck.`,
      version: "1.0.0",
      digest: "d",
      aktiv,
      ausloeser: modell ? [] : ["de.mako.process.failed"],
      modell,
      liest: [{ werkzeug: "makod/get_process", wozu: "Die Prozesse." }],
      nur_lesend: true,
      mit_freigabe: 0,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          faehigkeiten: [
            skill("desk-assistant", "Assistent im Desk", true, "anthropic/claude-sonnet-5"),
            skill("deadline-alert-agent", "Fristen-Wächter", true, null),
            skill("eeg-agent", "EEG-Anlagen", false, "anthropic/claude-sonnet-5"),
          ],
        }),
      ),
    );
    await open("/assistent");
    expect(await screen.findByText("Assistent im Desk")).toBeTruthy();
    expect(screen.getByText("Der Assistent liest nur.")).toBeTruthy();
    expect(screen.getByText("regelbasiert, kein Modell")).toBeTruthy();
    expect(screen.getByText("startet auf Ihre Frage")).toBeTruthy();
    // A skill's reach is shown by service, never as the service's tool names.
    expect(screen.getAllByText("Marktkommunikation")).toHaveLength(2);
    expect(screen.queryByText("EEG-Anlagen")).toBeNull();
    fireEvent.click(screen.getByRole("switch", { name: "Nur aktive" }));
    expect(await screen.findByText("EEG-Anlagen")).toBeTruthy();
    expect(screen.getByText("nicht aktiviert")).toBeTruthy();
  });

  it("files a skill's Hinweis under its MaLo and mutes the skill on request", async () => {
    localStorage.removeItem("mako-desk.ausgeblendet");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) => {
        if (url === "/svc/agentd/api/v1/hinweise") {
          return Response.json([
            {
              task: "t-1",
              run: "r-1",
              art: "deadline.breach",
              skill: "deadline-alert-agent",
              titel: "Fristen-Wächter",
              text: "1 Marktprozess nach der Frist. Die Frist ist abgelaufen.",
              prioritaet: "urgent",
              faellig: "2031-01-02T10:00:00Z",
              anker: [{ art: "malo", wert: "51238696012" }],
            },
          ]);
        }
        return new Response("{}", { status: 503 });
      }),
    );
    await open("/ueberblick");
    expect(
      await screen.findByText("1 Marktprozess nach der Frist. Die Frist ist abgelaufen."),
    ).toBeTruthy();
    expect(screen.getByText("dringend")).toBeTruthy();
    expect(screen.getByRole("link", { name: /MaLo 51238696012/ }).getAttribute("href")).toBe(
      "/prozess?key=51238696012",
    );
    fireEvent.click(screen.getByRole("button", { name: "Fristen-Wächter ausblenden" }));
    // Muted, not gone: the section says so and brings it back in one click.
    expect(
      screen.queryByText("1 Marktprozess nach der Frist. Die Frist ist abgelaufen."),
    ).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "1 ausgeblendet · zeigen" }));
    expect(
      screen.getByText("1 Marktprozess nach der Frist. Die Frist ist abgelaufen."),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Fristen-Wächter wieder einblenden" })).toBeTruthy();
    localStorage.removeItem("mako-desk.ausgeblendet");
  });

  it("prints every shortcut on ⌘/, from the table the handlers match", async () => {
    await open("/prozess");
    await screen.findByRole("heading", { name: "Prozesse", level: 1 });
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "/", metaKey: true }));
    });
    const sheet = await screen.findByRole("dialog", { name: "Tastaturkürzel" });
    for (const s of SHORTCUTS) expect(within(sheet).getByText(s.keys)).toBeTruthy();
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "/", metaKey: true }));
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("sets the appearance from the account menu and back to the system", async () => {
    await open("/prozess");
    fireEvent.click(await screen.findByRole("button", { name: /Konto und Erscheinungsbild/ }));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Dunkel" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    fireEvent.click(await screen.findByRole("button", { name: /Konto und Erscheinungsbild/ }));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "System" }));
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });

  it("renders the gallery in both appearances", async () => {
    await open("/dev/gallery");
    expect(await screen.findByText("Hell")).toBeTruthy();
    expect(screen.getByText("Dunkel")).toBeTruthy();
    expect(screen.getAllByText("Freigeben")).toHaveLength(2);
  });
});
