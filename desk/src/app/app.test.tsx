import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { vergessen } from "../data/verweigert";
import { OHNE_WARUM, QUELLE_IDS } from "../inbox/model";
import { QUELLEN } from "../inbox/quellen";
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
  vergessen();
  localStorage.removeItem("mako-desk.rolle");
});

describe("the Desk", () => {
  it("opens on Heute with the Rolle badge in the toolbar", async () => {
    await open("/");
    expect(await screen.findByRole("heading", { name: "Heute", level: 1 })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Rolle: Netz/ })).toBeTruthy();
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
    localStorage.setItem("mako-desk.rolle", "lieferant");
    await open("/heute");
    await screen.findByRole("list", { name: "Später" });
    const band = (name: string) =>
      within(screen.getByRole("list", { name }))
        .getAllByRole("listitem")
        .map((r) => r.textContent ?? "");
    // The names arrive in one vertragd read after the order.
    await waitFor(() => expect(band("Später")[0]).toContain("Früh"));
    const spaeter = band("Später");
    expect(spaeter).toHaveLength(2);
    expect(spaeter[0]).toContain("Früh");
    expect(spaeter[1]).toContain("Spät");
    const ohne = band("Ohne Frist");
    expect(ohne).toHaveLength(1);
    expect(ohne[0]).toContain("keine Frist");
    // A page of rows costs one read for the order and one for the names —
    // never a call per row, however many rows a tenant holds.
    expect(calls.filter((c) => c === "list_open_processes")).toHaveLength(1);
    expect(calls.filter((c) => c === "kunden_zu_malos")).toHaveLength(1);
    expect(calls.filter((c) => c === "get_process")).toHaveLength(0);
    // processd has more than the page: the source line says so.
    expect(
      within(screen.getByRole("list", { name: "Quellen" })).getByText("Anmeldungen: 3+"),
    ).toBeTruthy();
  });

  it("merges every source by Frist, files a Hinweis under its item and names a failed source", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/svc/agentd/api/v1/hinweise") {
          const h = (task: string, wert: string, text: string) => ({
            task,
            run: `r-${task}`,
            art: "triage",
            skill: "deadline-alert-agent",
            titel: "Fristen-Wächter",
            text,
            prioritaet: "normal",
            faellig: "2031-01-03T09:00:00Z",
            anker: [{ art: "malo", wert }],
          });
          return Response.json([
            h("t-1", "41203768152", "Die Antwortfrist ist seit gestern abgelaufen."),
            h("t-2", "30871225644", "Für diese MaLo fehlt die Zuordnung."),
          ]);
        }
        if (url.startsWith("/svc/agentd/")) return Response.json({ faehigkeiten: [] });
        const m = JSON.parse(String(init?.body));
        let result: unknown = {};
        if (m.method === "tools/call" && m.params.name === "list_overdue_processes") {
          result = {
            structuredContent: {
              overdue: [
                {
                  process_id: "p-alt",
                  pid: 55001,
                  family: "gpke",
                  state: "running",
                  malo_id: "41203768152",
                  deadline_at: "2020-01-01T10:00:00Z",
                  deadline_source: "Antwort auf die Anmeldung",
                },
              ],
              count: 1,
              saturated: false,
              limit: 500,
            },
          };
        } else if (m.method === "tools/call" && m.params.name === "list_pending_approvals") {
          result = { structuredContent: { freigaben: [], count: 0, saturated: false } };
        } else if (m.method === "tools/call" && m.params.name === "list_korrekturbedarf") {
          result = {
            isError: true,
            content: [{ type: "text", text: "read-mabis-run denied for this principal" }],
          };
        }
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/heute");
    const ueber = within(await screen.findByRole("list", { name: "Überfällig" }));
    expect(ueber.getByText("Antwort auf die Anmeldung")).toBeTruthy();
    // The Hinweis on the overdue MaLo sits under that item …
    expect(ueber.getByText("Die Antwortfrist ist seit gestern abgelaufen.")).toBeTruthy();
    // … and the one no item carries is an item of its own, at its own Frist.
    expect(await screen.findByText("Für diese MaLo fehlt die Zuordnung.")).toBeTruthy();
    expect(
      screen.getByRole("list", { name: "Überfällig" }).textContent?.includes("fehlt die Zuordnung"),
    ).toBe(false);
    // A refused source names itself in the service's words; the rest still lists.
    expect(
      await within(screen.getByRole("list", { name: "Quellen" })).findByText(
        /Korrekturbedarf: read-mabis-run denied/,
      ),
    ).toBeTruthy();
    // The assistant is asked about the item in focus.
    expect(screen.getByRole("heading", { name: /Fragen zu/ })).toBeTruthy();
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
    localStorage.setItem("mako-desk.rolle", "lieferant");
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
    const calls: string[] = [];
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
        } else if (m.method === "tools/call" && url.includes("/svc/vertragd/")) {
          result = { structuredContent: { anmeldungen: [], gesamt: 1284 } };
        } else if (m.method === "tools/call" && url.includes("/svc/billingd/")) {
          result = { structuredContent: { records: [], count: 50, saturated: true } };
        } else if (m.method === "tools/call" && url.includes("/svc/invoicd/")) {
          result = { structuredContent: { disputes: [], count: 3, saturated: false } };
        } else if (m.method === "tools/call" && url.includes("/svc/accountingd/")) {
          result = { structuredContent: { buchungen: [], count: 120, saturated: true } };
        }
        if (m.method === "tools/call") calls.push(`${url.split("/")[2]}/${m.params.name}`);
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
    const pruefung = await screen.findByRole("link", { name: "Rechnungsprüfung: 50+" });
    expect(pruefung.getAttribute("href")).toBe("/heute");
    expect(
      (await screen.findByRole("link", { name: "Rechnungsdispute: 3" })).getAttribute("href"),
    ).toBe("/heute");
    // accountingd's count is every parked booking, not the page it sent: exact
    // even when the page is full.
    expect(
      (await screen.findByRole("link", { name: "Geparkte Buchungen: 120" })).getAttribute("href"),
    ).toBe("/heute");
    // One read per tile, and none of another Rolle's sources.
    for (const tool of [
      "accountingd/list_parked_bookings",
      "billingd/list_review_queue",
      "invoicd/list_disputes",
      "processd/list_overdue_processes",
      "vertragd/list_anmeldungen",
    ])
      expect(calls.filter((c) => c === tool)).toHaveLength(1);
    expect(calls.filter((c) => c.startsWith("makod/") || c.startsWith("edmd/"))).toEqual([]);
    localStorage.removeItem("mako-desk.rolle");
  });

  it("counts every dead letter on the Netz Überblick, and one failed source fails only its tile", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init: RequestInit) => {
        if (url.includes("/svc/processd/")) return new Response("down", { status: 503 });
        const m = JSON.parse(String(init.body));
        const result =
          m.method === "tools/call"
            ? {
                structuredContent: {
                  dead_letters: [
                    {
                      rejected_at: "2026-11-07T09:15:00Z",
                      reason_label: "unknown_pid",
                      reason_detail: "PID 99999 is not registered",
                    },
                  ],
                  gesamt: 23,
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/ueberblick");
    // The total the service holds, not the one row of the page it sent.
    const dead = await screen.findByRole("link", { name: "Dead Letters: 23" });
    expect(dead.getAttribute("href")).toBe("/dead-letters");
    expect(
      await screen.findByRole("link", {
        name: "Fristen überschritten: Prozessüberwachung nicht erreichbar",
      }),
    ).toBeTruthy();
  });

  it("lists the dead letters the service returned and says the page is not all of them", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const m = JSON.parse(String(init.body));
        const result =
          m.method === "tools/call"
            ? {
                structuredContent: {
                  dead_letters: [
                    {
                      rejected_at: "2026-11-07T09:15:00Z",
                      reason_label: "unknown_pid",
                      reason_detail: "PID 99999 is not registered",
                    },
                    {
                      rejected_at: null,
                      reason_label: "outbox_exhausted",
                      reason_detail: "delivery to 9900357000004 exhausted",
                    },
                  ],
                  gesamt: 23,
                },
              }
            : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    await open("/dead-letters");
    expect(await screen.findByText("PID 99999 is not registered")).toBeTruthy();
    // 09:15 UTC is 10:15 in Berlin (CET).
    expect(screen.getByText("07.11.2026, 10:15")).toBeTruthy();
    expect(screen.getByText("Zeitpunkt unlesbar")).toBeTruthy();
    expect(screen.getByText(/Die jüngsten 2 von 23/)).toBeTruthy();
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

    // „Nein" shows at once and is held, not sent; the reason given inside the
    // hold replaces it, and one verdict leaves when the page lets it go.
    fireEvent.click(screen.getByRole("button", { name: "Nein" }));
    expect(screen.getByText("Als nicht hilfreich bewertet")).toBeTruthy();
    expect(verdicts).toEqual([]);
    const warum = screen.getByRole("textbox", { name: "Was fehlte?" });
    fireEvent.change(warum, { target: { value: "Die Frist des LF fehlt." } });
    fireEvent.submit(warum.closest("form") as HTMLFormElement);
    expect(screen.getByText("Begründung ergänzt")).toBeTruthy();
    expect(screen.queryByText("Als nicht hilfreich bewertet")).toBeNull();
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() =>
      expect(verdicts).toEqual([{ hilfreich: false, begruendung: "Die Frist des LF fehlt." }]),
    );
    expect(await screen.findByText("Danke — das bleibt beim Lauf.")).toBeTruthy();
  });

  it("hands a question from ⌘K to the assistant on screen", async () => {
    const asked: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
        if (url === "/svc/agentd/api/v1/runs") {
          asked.push(JSON.parse(String(init?.body)));
          return Response.json([{ agent_name: "desk-assistant", run_id: "r-7" }], { status: 202 });
        }
        if (url.startsWith("/svc/agentd/")) {
          return Response.json({
            run: "r-7",
            status: "running",
            disclosure: "AI-generated.",
            saetze: [],
            vorschlag: null,
            schritte: [],
            hilfreich: null,
          });
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
    await screen.findByRole("region", { name: "Fragen" });
    // The palette offers the row while a panel is mounted, and hands the
    // question over (typing into ⌘K is driven in a browser by the screenshots).
    const { frageMoeglich, frageStellen } = await import("./frage");
    expect(frageMoeglich()).toBe(true);
    act(() => frageStellen("Warum hängt die Anmeldung?"));
    await waitFor(() =>
      expect(asked).toEqual([
        {
          agent: "desk-assistant",
          event_type: "manual",
          input: { frage: "Warum hängt die Anmeldung?", malo: "51238696012" },
        },
      ]),
    );
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
    /** The proposal agentd computed for q-1 unasked, as `/vorschlaege` answers it. */
    wartend?: Record<string, unknown>;
    /** processd's answer to the decision, when it is not a plain 200. */
    entscheidung?: Response;
    /** The skills agentd's catalog lists. */
    katalog?: Record<string, unknown>[];
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
        if (url.includes("/api/v1/queue/") && opts.entscheidung) return opts.entscheidung;
        return new Response(null, { status: url.endsWith("/bewertung") ? 204 : 200 });
      }
      if (url === "/svc/agentd/api/v1/agents/catalog") {
        return Response.json({ faehigkeiten: opts.katalog ?? [] });
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
      if (url.startsWith("/svc/agentd/api/v1/vorschlaege")) {
        return Response.json({ freigabe: { "q-1": opts.wartend ?? null } });
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

  it("chooses from the keyboard and names who decided first when the race is lost", async () => {
    const posts: { url: string; body: unknown }[] = [];
    vi.stubGlobal(
      "fetch",
      freigabeFetch({
        posts,
        entscheidung: Response.json(
          {
            error: "Conflict",
            detail: "Freigabe q-1 is already Approved",
            code: "already_decided",
            status: "Approved",
            decided_by: "erika",
            decided_at: "2031-01-02T08:15:00Z",
          },
          { status: 409 },
        ),
      }),
    );
    await open("/freigaben");
    await screen.findByRole("heading", { name: /verbundenen Lieferanten/ });
    // R chooses „Ablehnen" — not while typing in a field.
    fireEvent.keyDown(window, { key: "r" });
    expect((screen.getByRole("radio", { name: "Ablehnen" }) as HTMLInputElement).checked).toBe(
      true,
    );
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    expect(await screen.findByText("Schon entschieden")).toBeTruthy();
    expect(
      screen.getByText(/erika hat diese Freigabe am 02\.01\.2031, 09:15 Uhr freigegeben/),
    ).toBeTruthy();
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

  it("shows the proposal waiting under a Freigabe and U takes it in, sending nothing", async () => {
    const posts: { url: string; body: unknown }[] = [];
    const fetch = freigabeFetch({
      posts,
      wartend: {
        run: "r-7",
        status: "succeeded",
        disclosure: "AI-generated.",
        saetze: [],
        vorschlag: {
          aktion: "ablehnen",
          antwortcode: "A02",
          begruendung: "Die MaLo nimmt nicht an der Marktkommunikation teil.",
          quelle: "processd/get_queue_entry",
          belegt: true,
        },
        schritte: [],
        hilfreich: null,
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    const zeile = await screen.findByRole("button", { name: /verbundenen Lieferanten/ });
    // Waiting under the row, marked as the model's, before anyone asked.
    expect(
      await screen.findByText("Die MaLo nimmt nicht an der Marktkommunikation teil."),
    ).toBeTruthy();
    expect(screen.getByText(/AI-generated\. Vorschlag: Ablehnen/)).toBeTruthy();
    expect(fetch.mock.calls.some(([u]) => u === "/svc/agentd/api/v1/runs")).toBe(false);
    zeile.focus();
    fireEvent.keyDown(window, { key: "u" });
    expect(zeile.getAttribute("aria-expanded")).toBe("true");
    await waitFor(() =>
      expect((screen.getByRole("radio", { name: /A02/ }) as HTMLInputElement).checked).toBe(true),
    );
    expect((screen.getByRole("radio", { name: "Ablehnen" }) as HTMLInputElement).checked).toBe(
      true,
    );
    expect(screen.getByText(/Vom Assistenten vorgeschlagen/)).toBeTruthy();
    expect(posts).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A02", antwort_tree: "E_0622" },
        },
        { url: "/svc/agentd/api/v1/runs/r-7/bewertung", body: { vorschlag: "sent" } },
      ]),
    );
  });

  it("decides a Freigabe in place from Heute, without leaving the list", async () => {
    const posts: { url: string; body: unknown }[] = [];
    vi.stubGlobal("fetch", freigabeFetch({ posts }));
    await open("/heute");
    const zeile = await screen.findByRole("button", { name: /verbundenen Lieferanten/ });
    expect(zeile.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(zeile);
    expect(zeile.getAttribute("aria-expanded")).toBe("true");
    // The decision form opens under the row; Heute stays on screen.
    expect(screen.getByRole("heading", { name: "Heute", level: 1 })).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: "Ablehnen" }));
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/processd/api/v1/queue/q-1/reject",
          body: { antwort_code: "A02", antwort_tree: "E_0622" },
        },
      ]),
    );
    expect(screen.getByRole("heading", { name: "Heute", level: 1 })).toBeTruthy();
  });

  /** Choose „Ablehnen" with A02 and send it past the confirm. */
  async function ablehnenSenden() {
    await screen.findByRole("heading", { name: /verbundenen Lieferanten/ });
    fireEvent.click(screen.getByRole("radio", { name: "Ablehnen" }));
    fireEvent.click(screen.getByRole("radio", { name: /A02/ }));
    fireEvent.click(screen.getByRole("button", { name: "Ablehnen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
  }

  it("shows processd's own sentence when it refuses a decision, not the status", async () => {
    vi.stubGlobal(
      "fetch",
      freigabeFetch({
        posts: [],
        entscheidung: Response.json(
          { error: "Unprocessable Entity", detail: "A02 is not a code of E_0623" },
          { status: 422 },
        ),
      }),
    );
    await open("/freigaben");
    await ablehnenSenden();
    expect(await screen.findByText("A02 is not a code of E_0623")).toBeTruthy();
    expect(screen.queryByText(/Unprocessable Entity/)).toBeNull();
    // A refusal of this one request leaves the controls in place.
    expect(screen.getByRole("radio", { name: "Ablehnen" })).toBeTruthy();
  });

  it("puts processd's reason where the controls were once its policy refuses a decision", async () => {
    vi.stubGlobal(
      "fetch",
      freigabeFetch({
        posts: [],
        entscheidung: Response.json(
          { error: "Forbidden", detail: "Reject denied for this principal" },
          { status: 403 },
        ),
      }),
    );
    await open("/freigaben");
    await ablehnenSenden();
    expect(await screen.findByText("Reject denied for this principal")).toBeTruthy();
    expect(screen.getByText(/lässt Sie hier nicht entscheiden/)).toBeTruthy();
    expect(screen.queryByRole("radio", { name: "Ablehnen" })).toBeNull();
    // R chooses nothing: no control is left to choose.
    fireEvent.keyDown(window, { key: "r" });
    expect(screen.queryByRole("radio")).toBeNull();
  });

  it("holds a discarded proposal: Rückgängig sends nothing, pagehide sends it", async () => {
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
    fireEvent.click(await screen.findByRole("button", { name: "Verwerfen" }));
    expect(screen.queryByText("Die MaLo nimmt nicht an der Marktkommunikation teil.")).toBeNull();
    expect(screen.getByText("Vorschlag verworfen")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Rückgängig/ }));
    expect(screen.getByText("Die MaLo nimmt nicht an der Marktkommunikation teil.")).toBeTruthy();
    expect(screen.queryByText("Vorschlag verworfen")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Verwerfen" }));
    expect(posts).toEqual([]);
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() =>
      expect(posts).toEqual([
        { url: "/svc/agentd/api/v1/runs/r-9/bewertung", body: { vorschlag: "dismissed" } },
      ]),
    );
  });

  /**
   * The Lieferant's Heute with one Rechnungsdisput and two Prüfposten — one
   * HELD, one under REVIEW — and a fetch recording every write.
   */
  function rechnungFetch(opts: {
    posts: { url: string; body: unknown }[];
    abgesendet?: boolean;
    /** The proposal agentd computed for the dispute unasked. */
    wartend?: Record<string, unknown>;
    /** A service's answer to a write, when it is not a plain 204. */
    antwort?: (url: string) => Response | undefined;
    /** The parked bookings accountingd lists. */
    geparkt?: Record<string, unknown>[];
    /** agentd's `/vorschlaege` answer, when it is not the dispute's proposal. */
    vorschlaege?: Record<string, unknown>;
    /** The skills agentd's catalog lists. */
    katalog?: Record<string, unknown>[];
    /** Every run a question started, with its input. */
    runs?: unknown[];
    /** The answer each started run reads, by its run id (`r-w1`, `r-w2`, …). */
    antworten?: Record<string, Record<string, unknown>>;
  }) {
    const disput = {
      id: "rc-1",
      process_id: "p-inv",
      pid: 31002,
      sender_mp_id: "9900357000004",
      rechnungsnummer: "NB-2031-0815",
      received_at: "2031-01-01T08:00:00Z",
      bo4e_version: "v202607",
      frist: "2031-01-20T23:00:00Z",
      abgesendet: opts.abgesendet ?? false,
      befunde: [
        {
          art: "TariffDeviation",
          bezeichnung: "Preis weicht vom Preisblatt ab",
          text: "Position 1 (Arbeitspreis): Einzelpreis 0,31200 EUR/kWh, das Preisblatt nennt 0,29800 EUR/kWh.",
        },
      ],
    };
    const posten = (id: string, band: "HELD" | "REVIEW", code: string) => ({
      id,
      malo_id: "51238696012",
      lf_mp_id: "9900357000004",
      rechnungsnummer: `R-${id}`,
      category: "STROM",
      period_from: "2031-01-01",
      period_to: "2031-01-31",
      total_brutto_eur: "119.00",
      risk_band: band,
      risk_score: 60,
      risk_findings: [{ code, weight: 40, message: `Befund ${code}` }],
      created_at: "2031-02-01T08:00:00Z",
    });
    return vi.fn(async (url: string, init?: RequestInit) => {
      if (
        url.startsWith("/svc/invoicd/api/") ||
        url.startsWith("/svc/billingd/api/") ||
        url.startsWith("/svc/accountingd/api/")
      ) {
        opts.posts.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null });
        return opts.antwort?.(url) ?? new Response(null, { status: 204 });
      }
      if (url.endsWith("/bewertung")) {
        opts.posts.push({ url, body: JSON.parse(String(init?.body)) });
        return new Response(null, { status: 204 });
      }
      if (url.startsWith("/svc/agentd/api/v1/vorschlaege")) {
        return Response.json(opts.vorschlaege ?? { disput: { "rc-1": opts.wartend ?? null } });
      }
      if (url === "/svc/agentd/api/v1/agents/catalog")
        return Response.json({ faehigkeiten: opts.katalog ?? [] });
      if (url === "/svc/agentd/api/v1/runs" && init?.method === "POST") {
        const runs = opts.runs ?? [];
        runs.push(JSON.parse(String(init.body)));
        return Response.json([{ agent_name: "desk-assistant", run_id: `r-w${runs.length}` }], {
          status: 202,
        });
      }
      const gelesen = /^\/svc\/agentd\/api\/v1\/runs\/(r-w\d+)\/antwort$/.exec(url);
      if (gelesen) return Response.json(opts.antworten?.[gelesen[1] ?? ""] ?? null);
      if (url.startsWith("/svc/agentd/")) return Response.json([]);
      const m = JSON.parse(String(init?.body));
      const name = m.method === "tools/call" ? m.params.name : null;
      const result =
        name === "partner_namen"
          ? {
              structuredContent: {
                partner: m.params.arguments.mp_ids.includes("9900357000004")
                  ? [{ mp_id: "9900357000004", name: "Stadtwerke Musterstadt Netz" }]
                  : [],
              },
            }
          : name === "list_disputes"
            ? { structuredContent: { disputes: [disput], count: 1, saturated: false } }
            : name === "list_review_queue"
              ? {
                  structuredContent: {
                    records: [
                      posten("b-held", "HELD", "AMOUNT_SPIKE"),
                      posten("b-rev", "REVIEW", "TAX_GAP"),
                    ],
                    count: 2,
                    saturated: false,
                  },
                }
              : name === "list_parked_bookings"
                ? {
                    structuredContent: {
                      buchungen: opts.geparkt ?? [],
                      count: opts.geparkt?.length ?? 0,
                      saturated: false,
                    },
                  }
                : name === "list_pending_approvals"
                  ? { structuredContent: { freigaben: [], count: 0, saturated: false } }
                  : name === "list_open_processes"
                    ? { structuredContent: { prozesse: [], weiter: null } }
                    : name === "list_overdue_processes"
                      ? {
                          structuredContent: {
                            overdue: [],
                            count: 0,
                            saturated: false,
                            limit: 500,
                          },
                        }
                      : name === "list_anmeldungen"
                        ? { structuredContent: { anmeldungen: [], gesamt: 0 } }
                        : name === "kunden_zu_malos"
                          ? { structuredContent: { kunden: [] } }
                          : name
                            ? { structuredContent: {} }
                            : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    });
  }

  it("settles a Disput in place: Beilegen is held for undo and sends no market message", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown }[] = [];
    const fetch = rechnungFetch({ posts, abgesendet: true });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    // Titled by who sent it, named by the Marktstammdaten in one read; the
    // Rechnungsnummer and the MP-ID beneath.
    const zeile = await screen.findByRole("button", { name: /Stadtwerke Musterstadt Netz/ });
    expect(zeile.textContent).toContain("Rechnung NB-2031-0815");
    expect(
      fetch.mock.calls.filter(([u, i]) => {
        if (!u.startsWith("/svc/marktd/")) return false;
        const m = JSON.parse(String(i?.body));
        return m.method === "tools/call" && m.params.name === "partner_namen";
      }),
    ).toHaveLength(1);
    fireEvent.click(zeile);
    expect(zeile.getAttribute("aria-expanded")).toBe("true");
    // The finding the dispute rests on, by its German name, and a refusal
    // that already went out. The checker's identifier is never shown.
    expect(
      within(screen.getByRole("list", { name: "Befunde" })).getByText(
        "Preis weicht vom Preisblatt ab",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/TariffDeviation/)).toBeNull();
    expect(
      (screen.getByRole("radio", { name: "Ablehnung senden" }) as HTMLInputElement).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("radio", { name: "Beilegen" }));
    fireEvent.change(screen.getByRole("textbox", { name: /Wie wurde er geklärt/ }), {
      target: { value: "NB schickt eine korrigierte Rechnung" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Beilegen" }));
    // Held, not sent, and no confirm: nothing goes to the market partner.
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("Disput beigelegt")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Rückgängig/ }));
    expect(posts).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Beilegen" }));
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/invoicd/api/v1/receipts/rc-1/resolve-dispute",
          body: { note: "NB schickt eine korrigierte Rechnung" },
        },
      ]),
    );
    localStorage.removeItem("mako-desk.rolle");
  });

  it("takes the proposal waiting on a Disput in, with its finding, and confirms the REMADV", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown }[] = [];
    const fetch = rechnungFetch({
      posts,
      wartend: {
        run: "r-d",
        status: "succeeded",
        disclosure: "AI-generated.",
        saetze: [],
        vorschlag: {
          aktion: "aufrechterhalten",
          antwortcode: null,
          befund: "TariffDeviation",
          begruendung: "Der Arbeitspreis weicht vom Preisblatt ab.",
          quelle: "invoicd/get_check_result",
          belegt: true,
        },
        schritte: [],
        hilfreich: null,
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    const zeile = await screen.findByRole("button", { name: /Stadtwerke Musterstadt Netz/ });
    // Waiting under the row, the model's and marked so, with its finding by
    // its German name — never the checker's identifier.
    expect(await screen.findByText(/AI-generated\. Vorschlag: Aufrechterhalten/)).toBeTruthy();
    expect(screen.getByText("wegen Preis weicht vom Preisblatt ab")).toBeTruthy();
    expect(screen.queryByText(/TariffDeviation/)).toBeNull();
    expect(fetch.mock.calls.some(([u]) => u === "/svc/agentd/api/v1/runs")).toBe(false);
    zeile.focus();
    fireEvent.keyDown(window, { key: "u" });
    await waitFor(() =>
      expect(
        (screen.getByRole("radio", { name: "Ablehnung senden" }) as HTMLInputElement).checked,
      ).toBe(true),
    );
    expect(screen.getByText(/Vom Assistenten vorgeschlagen/)).toBeTruthy();
    expect(screen.queryByText(/TariffDeviation/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Ablehnung senden …" }));
    expect(posts).toEqual([]);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    await waitFor(() =>
      expect(posts).toEqual([
        { url: "/svc/invoicd/api/v1/receipts/rc-1/dispatch-answer", body: null },
        { url: "/svc/agentd/api/v1/runs/r-d/bewertung", body: { vorschlag: "sent" } },
      ]),
    );
    localStorage.removeItem("mako-desk.rolle");
  });

  it("decides Prüfposten in place: a held invoice after the confirm, a reviewed one on a hold", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown }[] = [];
    vi.stubGlobal(
      "fetch",
      rechnungFetch({
        posts,
        antwort: (url) =>
          url.endsWith("/b-rev/reviewed")
            ? Response.json(
                { error: { code: "NOT_IN_REVIEW", message: "record is not awaiting review" } },
                { status: 409 },
              )
            : undefined,
      }),
    );
    await open("/heute");
    fireEvent.click(await screen.findByRole("button", { name: /Befund AMOUNT_SPIKE/ }));
    fireEvent.click(screen.getByRole("button", { name: "Freigeben …" }));
    expect(posts).toEqual([]);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt senden" }));
    await waitFor(() =>
      expect(posts).toEqual([{ url: "/svc/billingd/api/v1/billing/b-held/release", body: null }]),
    );

    fireEvent.click(await screen.findByRole("button", { name: /Befund TAX_GAP/ }));
    fireEvent.click(screen.getByRole("button", { name: "Geprüft" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("Als geprüft vermerkt")).toBeTruthy();
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    // The Abrechnung's own sentence, read out of its nested refusal.
    expect(await screen.findByText(/record is not awaiting review/)).toBeTruthy();
    expect(screen.getByText("Schon erledigt")).toBeTruthy();
    expect(posts.at(-1)).toEqual({
      url: "/svc/billingd/api/v1/billing/b-rev/reviewed",
      body: null,
    });
    localStorage.removeItem("mako-desk.rolle");
  });

  it("resolves a geparkte Buchung in place: rebooked on the date it fills in, discarded with a Grund, each confirmed", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown }[] = [];
    let versuche = 0;
    const geparkt = (key: string, beleg: string) => ({
      key,
      art: "RECHNUNG",
      malo_id: "51238696012",
      lf_mp_id: "9900357000004",
      betrag_ct: 900,
      buchungsdatum: "2031-01-31",
      beleg,
      code: 301,
      meldung: "period sealed through 2031-01-31",
      hinweis: "Die Periode ist festgeschrieben. Auf ein Datum im ersten offenen Monat neu buchen.",
      vorschlag_datum: "2031-02-01",
      seit: "2031-02-02T08:00:00Z",
    });
    const fetch = rechnungFetch({
      posts,
      geparkt: [geparkt("rechnung:R-1", "R-1"), geparkt("rechnung:R-2", "R-2")],
      antwort: (url) => {
        if (!url.endsWith("/rebook")) return undefined;
        versuche += 1;
        return versuche === 1
          ? Response.json(
              {
                error: "Unprocessable Entity",
                detail: "period sealed through 2031-02-28",
                code: "ledger.301",
              },
              { status: 422 },
            )
          : Response.json({ gebucht: true, entry_id: "e-1" }, { status: 201 });
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    // In words, by its amount and its refusal — not the ledger's code.
    const [erste, zweite] = await screen.findAllByRole("button", {
      name: /Rechnung 9,00\s€ — Periode festgeschrieben/,
    });
    if (!erste || !zweite) throw new Error("two parked bookings expected");
    expect(erste.textContent).toContain("Beleg R-1");
    expect(erste.textContent).toContain("Buchungsdatum 31.01.2031");
    fireEvent.click(erste);
    expect(erste.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(/Auf ein Datum im ersten offenen Monat neu buchen/)).toBeTruthy();
    // The first open day the service names is filled in, and stays editable.
    const datum = screen.getByLabelText(/Neu buchen am/) as HTMLInputElement;
    expect(datum.value).toBe("2031-02-01");
    fireEvent.click(screen.getByRole("button", { name: "Neu buchen …" }));
    expect(posts).toEqual([]);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt buchen" }));
    // The ledger's refusal, in its words, where the decision is.
    expect(await screen.findByText(/period sealed through 2031-02-28/)).toBeTruthy();
    expect(posts).toEqual([
      {
        url: "/svc/accountingd/api/v1/bookings/rechnung%3AR-1/rebook",
        body: { buchungsdatum: "2031-02-01" },
      },
    ]);
    fireEvent.change(datum, { target: { value: "2031-03-01" } });
    fireEvent.click(screen.getByRole("button", { name: "Neu buchen …" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt buchen" }));
    await waitFor(() => expect(erste.getAttribute("aria-expanded")).toBe("false"));
    expect(posts.at(-1)).toEqual({
      url: "/svc/accountingd/api/v1/bookings/rechnung%3AR-1/rebook",
      body: { buchungsdatum: "2031-03-01" },
    });

    // R chooses Verwerfen; it waits for a Grund, then for the confirm.
    fireEvent.click(zweite);
    fireEvent.keyDown(window, { key: "r" });
    const verwerfen = await screen.findByRole("button", { name: "Verwerfen …" });
    expect(verwerfen.hasAttribute("disabled")).toBe(true);
    fireEvent.change(screen.getByRole("textbox", { name: /Warum wird sie verworfen/ }), {
      target: { value: "Doppelt erfasst" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Verwerfen …" }));
    expect(posts).toHaveLength(2);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt verwerfen" }));
    await waitFor(() =>
      expect(posts.at(-1)).toEqual({
        url: "/svc/accountingd/api/v1/bookings/rechnung%3AR-2/discard",
        body: { grund: "Doppelt erfasst" },
      }),
    );
    localStorage.removeItem("mako-desk.rolle");
  });

  /** A parked booking accountingd lists, refused because its month is sealed. */
  const geparktFixture = (key: string, beleg: string) => ({
    key,
    art: "RECHNUNG",
    malo_id: "51238696012",
    lf_mp_id: "9900357000004",
    betrag_ct: 900,
    buchungsdatum: "2031-01-31",
    beleg,
    code: 301,
    meldung: "period sealed through 2031-01-31",
    hinweis: "Die Periode ist festgeschrieben. Auf ein Datum im ersten offenen Monat neu buchen.",
    vorschlag_datum: "2031-02-01",
    seit: "2031-02-02T08:00:00Z",
  });

  /** The desk-assistant in agentd's catalog, with its record over the window. */
  const assistentImKatalog = (bilanz: Record<string, unknown> | null) => ({
    name: "desk-assistant",
    titel: "Assistent im Desk",
    zweck: "Beantwortet Fragen.",
    version: "1.3.0",
    digest: "d",
    aktiv: bilanz !== null,
    ausloeser: [],
    modell: "anthropic/claude-sonnet-5",
    liest: [{ werkzeug: "makod/get_process", wozu: "Die Prozesse." }],
    nur_lesend: true,
    mit_freigabe: 0,
    bilanz,
  });

  const BILANZ = {
    tage: 30,
    laeufe: 17,
    offen: 1,
    abgeschlossen: { succeeded: 16 },
    hilfreich: { ja: 6, nein: 1 },
    vorschlag: { sent: 14, edited: 2, dismissed: 1 },
  };

  it("takes the proposal waiting on a geparkte Buchung into its decision, marked, sent only on the confirm", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const posts: { url: string; body: unknown }[] = [];
    const wartend = (
      run: string,
      vorschlag: Record<string, unknown> | null,
      status = "succeeded",
    ) => ({
      run,
      status,
      disclosure: "AI-generated.",
      saetze: [],
      vorschlag,
      schritte: [],
      hilfreich: null,
    });
    const fetch = rechnungFetch({
      posts,
      geparkt: [
        geparktFixture("rechnung:R-1", "R-1"),
        geparktFixture("rechnung:R-2", "R-2"),
        geparktFixture("rechnung:R-3", "R-3"),
      ],
      katalog: [assistentImKatalog(BILANZ)],
      antwort: (url) =>
        url.endsWith("/rebook") ? Response.json({ gebucht: true }, { status: 201 }) : undefined,
      vorschlaege: {
        buchung: {
          "rechnung:R-1": wartend("r-b1", {
            aktion: "neu_buchen",
            buchungsdatum: "2031-02-03",
            grund: null,
            antwortcode: null,
            befund: null,
            begruendung: "Der Januar ist festgeschrieben; der erste offene Tag ist der 03.02.2031.",
            quelle: "accountingd/get_parked_booking",
            belegt: true,
          }),
          "rechnung:R-2": wartend("r-b2", {
            aktion: "verwerfen",
            buchungsdatum: null,
            grund: "R-2 ist am 14.01.2031 bereits gebucht.",
            antwortcode: null,
            befund: null,
            begruendung: "Das Hauptbuch führt R-2 mit demselben Betrag am 14.01.2031.",
            quelle: "accountingd/list_ledger",
            belegt: false,
          }),
          "rechnung:R-3": wartend("r-b3", null, "running"),
        },
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    const [erste, zweite, dritte] = await screen.findAllByRole("button", {
      name: /Rechnung 9,00\s€ — Periode festgeschrieben/,
    });
    if (!erste || !zweite || !dritte) throw new Error("three parked bookings expected");
    // Every booking on screen asked for in one read, never one per row.
    await screen.findByText(/AI-generated\. Vorschlag: Neu buchen/);
    const gelesen = fetch.mock.calls
      .map(([u]) => String(u))
      .filter((u) => u.startsWith("/svc/agentd/api/v1/vorschlaege"));
    expect(new Set(gelesen)).toEqual(
      new Set([
        "/svc/agentd/api/v1/vorschlaege?disput=rc-1&buchung=rechnung%3AR-1,rechnung%3AR-2,rechnung%3AR-3",
      ]),
    );
    expect(fetch.mock.calls.some(([u]) => u === "/svc/agentd/api/v1/runs")).toBe(false);

    // Neu buchen: chosen, the assistant's date filled in and marked so, with
    // its reason, its source and the record of its proposals.
    fireEvent.click(erste);
    const datum = (await screen.findByLabelText(/Neu buchen am/)) as HTMLInputElement;
    await waitFor(() => expect(datum.value).toBe("2031-02-03"));
    expect((screen.getByRole("radio", { name: "Neu buchen" }) as HTMLInputElement).checked).toBe(
      true,
    );
    expect(screen.getByText("Datum vom Assistenten vorgeschlagen.")).toBeTruthy();
    expect(screen.getByText(/der erste offene Tag ist der 03\.02\.2031\./)).toBeTruthy();
    expect(screen.getByText("Kundenkonten · geparkte Buchung")).toBeTruthy();
    expect(screen.getByText("In 30 Tagen: 14 von 17 Vorschlägen übernommen")).toBeTruthy();
    expect(posts).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Neu buchen …" }));
    expect(posts).toEqual([]);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt buchen" }));
    await waitFor(() =>
      expect(posts).toEqual([
        {
          url: "/svc/accountingd/api/v1/bookings/rechnung%3AR-1/rebook",
          body: { buchungsdatum: "2031-02-03" },
        },
        { url: "/svc/agentd/api/v1/runs/r-b1/bewertung", body: { vorschlag: "sent" } },
      ]),
    );

    // Verwerfen: the Grund filled in and marked; the unchecked Begründung
    // says so. Edited by the person, it is recorded as changed.
    fireEvent.click(zweite);
    const grund = (await screen.findByRole("textbox", {
      name: /Warum wird sie verworfen/,
    })) as HTMLTextAreaElement;
    await waitFor(() => expect(grund.value).toBe("R-2 ist am 14.01.2031 bereits gebucht."));
    expect(screen.getByText(/Grund vom Assistenten vorgeschlagen/)).toBeTruthy();
    expect(screen.getByText("nicht belegt")).toBeTruthy();
    fireEvent.change(grund, { target: { value: "Doppelt erfasst, R-2 steht am 14.01.2031." } });
    expect(screen.queryByText(/Grund vom Assistenten vorgeschlagen/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Verwerfen …" }));
    expect(posts).toHaveLength(2);
    fireEvent.click(await screen.findByRole("button", { name: "Jetzt verwerfen" }));
    await waitFor(() =>
      expect(posts.slice(2)).toEqual([
        {
          url: "/svc/accountingd/api/v1/bookings/rechnung%3AR-2/discard",
          body: { grund: "Doppelt erfasst, R-2 steht am 14.01.2031." },
        },
        { url: "/svc/agentd/api/v1/runs/r-b2/bewertung", body: { vorschlag: "edited" } },
      ]),
    );

    // Still being prepared: a quiet line, the service's own date kept.
    fireEvent.click(dritte);
    expect(await screen.findByText("Der Assistent prüft die Buchung …")).toBeTruthy();
    expect((screen.getByLabelText(/Neu buchen am/) as HTMLInputElement).value).toBe("2031-02-01");
    localStorage.removeItem("mako-desk.rolle");
  });

  it("answers „Warum?“ beside the item, from ? on the selected row or the button", async () => {
    localStorage.setItem("mako-desk.rolle", "lieferant");
    const runs: { agent: string; input: Record<string, string> }[] = [];
    const antwort = (run: string, text: string) => ({
      run,
      status: "succeeded",
      disclosure: "AI-generated.",
      saetze: [
        { text, quelle: "accountingd/get_parked_booking", belegt: true },
        {
          text: "Gebucht wird sie am 01.02.2031.",
          quelle: "accountingd/get_parked_booking",
          belegt: false,
        },
      ],
      vorschlag: null,
      schritte: [{ quelle: "accountingd/get_parked_booking", fertig: true }],
      hilfreich: null,
    });
    const fetch = rechnungFetch({
      posts: [],
      geparkt: [geparktFixture("rechnung:R-1", "R-1")],
      vorschlaege: {},
      runs,
      antworten: {
        "r-w1": antwort("r-w1", "Das Hauptbuch hat R-1 für den 31.01.2031 abgelehnt."),
        "r-w2": antwort("r-w2", "Der Disput rc-1 wartet auf Ihre Entscheidung."),
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/heute");
    const buchung = await screen.findByRole("button", {
      name: /Rechnung 9,00\s€ — Periode festgeschrieben/,
    });
    // Each row whose record the assistant reads offers it; the Prüfposten,
    // whose record it does not, offers none.
    const zeile = (el: HTMLElement) => el.closest("li") as HTMLElement;
    expect(within(zeile(buchung)).getByRole("button", { name: "Warum?" })).toBeTruthy();
    const posten = screen.getAllByRole("button", { name: /Befund AMOUNT_SPIKE/ })[0] as HTMLElement;
    expect(within(zeile(posten)).queryByRole("button", { name: "Warum?" })).toBeNull();

    act(() => buchung.focus());
    fireEvent.keyDown(window, { key: "?", shiftKey: true });
    const antwortFeld = await within(zeile(buchung)).findByRole("region", {
      name: "Warum steht das hier?",
    });
    expect(
      await within(antwortFeld).findByText("Das Hauptbuch hat R-1 für den 31.01.2031 abgelehnt."),
    ).toBeTruthy();
    expect(runs).toEqual([
      {
        agent: "desk-assistant",
        event_type: "manual",
        input: {
          frage: "Warum steht das auf meiner Liste, und was entscheidet es?",
          buchung: "rechnung:R-1",
        },
      },
    ]);
    // The disclosure first, every sentence with its source and its check, and „Hilfreich?".
    expect(within(antwortFeld).getByText("AI-generated.")).toBeTruthy();
    expect(within(antwortFeld).getAllByText("Kundenkonten · geparkte Buchung")).toHaveLength(2);
    expect(within(antwortFeld).getAllByText("nicht belegt")).toHaveLength(1);
    expect(within(antwortFeld).getByRole("group", { name: "Hilfreich?" })).toBeTruthy();
    // ? again puts it away; asking again reads the same run.
    fireEvent.keyDown(window, { key: "?", shiftKey: true });
    await waitFor(() =>
      expect(
        within(zeile(buchung)).queryByRole("region", { name: "Warum steht das hier?" }),
      ).toBeNull(),
    );
    fireEvent.keyDown(window, { key: "?", shiftKey: true });
    expect(
      await screen.findByText("Das Hauptbuch hat R-1 für den 31.01.2031 abgelehnt."),
    ).toBeTruthy();
    expect(runs).toHaveLength(1);

    // The button asks about its own row, with that row's Anker.
    const disput = screen.getByRole("button", { name: /Stadtwerke Musterstadt Netz/ });
    fireEvent.click(within(zeile(disput)).getByRole("button", { name: "Warum?" }));
    expect(await screen.findByText("Der Disput rc-1 wartet auf Ihre Entscheidung.")).toBeTruthy();
    expect(runs[1]?.input).toEqual({
      frage: "Warum steht das auf meiner Liste, und was entscheidet es?",
      disput: "rc-1",
    });
    localStorage.removeItem("mako-desk.rolle");
  });

  it("offers „Warum?“ on every Heute source, or names why a source does not (SC-002)", async () => {
    // One item from every source, read by the Rolle that reads it.
    const zeilen = {
      list_pending_approvals: {
        freigaben: [
          {
            id: "q-1",
            process_id: "p-1",
            pid: 55001,
            malo_id: "51238696012",
            marktrolle: "NB",
            grund: "Anmeldung eines verbundenen Lieferanten",
            art: "eskaliert",
            antwort: null,
            codes_freigeben: null,
            codes_ablehnen: null,
            kann_freigeben: true,
            kann_ablehnen: true,
            status: "Pending",
            expires_at: "2031-01-02T09:00:00Z",
            created_at: "2031-01-01T08:00:00Z",
            decided_by: null,
            decided_at: null,
          },
        ],
        count: 1,
        saturated: false,
      },
      list_overdue_processes: {
        overdue: [
          {
            process_id: "p-ue",
            malo_id: null,
            pid: 55001,
            state: "Wartet",
            family: "GPKE",
            deadline_source: "GPKE Teil 2",
            deadline_at: "2031-01-02T10:00:00Z",
          },
        ],
        count: 1,
        saturated: false,
        limit: 500,
      },
      list_open_processes: {
        prozesse: [{ process_id: "p-an", malo_id: null, pid: 55001, deadline_at: null }],
        weiter: null,
      },
      list_anmeldungen: {
        anmeldungen: [
          {
            kunden_id: "k-1",
            malo_id: "51238696012",
            name: null,
            sparte: "STROM",
            vertrags_nr: "V-1",
          },
        ],
        gesamt: 1,
      },
      list_korrekturbedarf: {
        korrekturbedarf: [
          {
            pruefmitteilung_id: "pm-1",
            bilanzierungsgebiet_id: "BG-1",
            period_from: "2031-01-01",
            period_to: "2031-01-31",
          },
        ],
      },
      list_overdue_reading_orders: {
        auftraege: [
          {
            id: "a-1",
            malo_id: "51238696012",
            ausfuehrt_bis: "2031-01-01",
            status: "offen",
            fehlschlag_grund: null,
            anlass: "Turnusablesung",
            frist: "2031-01-02T10:00:00Z",
          },
        ],
        count: 1,
        saturated: false,
      },
      list_disputes: {
        disputes: [
          {
            id: "rc-1",
            process_id: "p-inv",
            pid: 31002,
            sender_mp_id: "9900357000004",
            rechnungsnummer: "NB-2031-0815",
            received_at: "2031-01-01T08:00:00Z",
            bo4e_version: "v202607",
            frist: "2031-01-20T23:00:00Z",
            abgesendet: false,
            befunde: [],
          },
        ],
        count: 1,
        saturated: false,
      },
      list_review_queue: {
        records: [
          {
            id: "b-1",
            malo_id: "51238696012",
            lf_mp_id: "9900357000004",
            rechnungsnummer: "R-b-1",
            category: "STROM",
            period_from: "2031-01-01",
            period_to: "2031-01-31",
            total_brutto_eur: "119.00",
            risk_band: "HELD",
            risk_score: 60,
            risk_findings: [{ code: "AMOUNT_SPIKE", weight: 40, message: "Befund AMOUNT_SPIKE" }],
            created_at: "2031-02-01T08:00:00Z",
          },
        ],
        count: 1,
        saturated: false,
      },
      list_parked_bookings: {
        buchungen: [geparktFixture("rechnung:R-1", "R-1")],
        count: 1,
        saturated: false,
      },
      partner_namen: { partner: [] },
      kunden_zu_malos: { kunden: [] },
    } as Record<string, unknown>;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/svc/agentd/api/v1/hinweise")
          return Response.json([
            {
              task: "t-h",
              run: "r-h",
              art: "deadline.breach",
              skill: "deadline-alert-agent",
              titel: "Fristen-Wächter",
              text: "Ein Prozess nach der Frist.",
              prioritaet: "normal",
              faellig: null,
              anker: [{ art: "process", wert: "p-h" }],
            },
          ]);
        if (url === "/svc/agentd/api/v1/agents/catalog") return Response.json({ faehigkeiten: [] });
        if (url.startsWith("/svc/agentd/")) return Response.json({});
        const m = JSON.parse(String(init?.body));
        const name = m.method === "tools/call" ? m.params.name : null;
        const result = name ? { structuredContent: zeilen[name] ?? {} } : {};
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
          headers: { "content-type": "application/json", "mcp-session-id": "s" },
        });
      }),
    );
    const bietet = new Map<string, boolean>();
    for (const [rolle, quellen] of Object.entries(QUELLEN)) {
      localStorage.setItem("mako-desk.rolle", rolle);
      await open("/heute");
      // Every source the Rolle reads has answered with its one item.
      await waitFor(() => {
        const da = new Set(
          [...document.querySelectorAll("li[data-quelle]")].map((li) =>
            li.getAttribute("data-quelle"),
          ),
        );
        for (const q of [...quellen, "hinweise"]) expect(da).toContain(q);
      });
      for (const li of document.querySelectorAll<HTMLElement>("li[data-quelle]")) {
        const q = li.getAttribute("data-quelle") ?? "";
        const offen = within(li).queryAllByRole("button", { name: "Warum?" }).length > 0;
        bietet.set(q, (bietet.get(q) ?? offen) && offen);
      }
      cleanup();
    }
    localStorage.removeItem("mako-desk.rolle");
    for (const q of QUELLE_IDS) {
      expect(bietet.has(q), `${q} appears on some Rolle's Heute`).toBe(true);
      const warumNicht = OHNE_WARUM[q];
      if (warumNicht) {
        expect(warumNicht.length, `${q} names its reason`).toBeGreaterThan(20);
        expect(bietet.get(q), `${q} is listed as not offering „Warum?“`).toBe(false);
      } else {
        expect(bietet.get(q), `${q} offers „Warum?“`).toBe(true);
      }
    }
  });

  it("shows on every proposal card one line of the assistant's proposal record", async () => {
    const posts: { url: string; body: unknown }[] = [];
    const fetch = freigabeFetch({
      posts,
      katalog: [assistentImKatalog(BILANZ)],
      wartend: {
        run: "r-7",
        status: "succeeded",
        disclosure: "AI-generated.",
        saetze: [],
        vorschlag: {
          aktion: "ablehnen",
          antwortcode: "A02",
          begruendung: "Die MaLo nimmt nicht an der Marktkommunikation teil.",
          quelle: "processd/get_queue_entry",
          belegt: true,
        },
        schritte: [],
        hilfreich: null,
      },
    });
    vi.stubGlobal("fetch", fetch);
    await open("/freigaben?id=q-1");
    expect(await screen.findByText("In 30 Tagen: 14 von 17 Vorschlägen übernommen")).toBeTruthy();
  });

  it("shows each skill's 30-day record on the Beipackzettel, a share only from five verdicts", async () => {
    const skill = (name: string, titel: string, bilanz: Record<string, unknown>) => ({
      ...assistentImKatalog(bilanz),
      name,
      titel,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          faehigkeiten: [
            skill("desk-assistant", "Assistent im Desk", BILANZ),
            skill("billing-agent", "Rechnungseinwände", {
              ...BILANZ,
              laeufe: 3,
              offen: 0,
              hilfreich: { ja: 1, nein: 1 },
              vorschlag: { sent: 1, dismissed: 2 },
            }),
            skill("eeg-agent", "EEG-Anlagen", {
              ...BILANZ,
              laeufe: 0,
              offen: 0,
              hilfreich: { ja: 0, nein: 0 },
              vorschlag: {},
            }),
          ],
        }),
      ),
    );
    await open("/assistent");
    const karte = async (titel: string) =>
      (await screen.findByText(titel)).closest("section") as HTMLElement;
    const record = (k: HTMLElement) =>
      within(k).getByRole("region", { name: "Bilanz der letzten 30 Tage" });

    const voll = record(await karte("Assistent im Desk"));
    expect(within(voll).getByText("In 30 Tagen")).toBeTruthy();
    expect(voll.textContent).toContain("17 · 1 laufen");
    expect(within(voll).getByText(/^86\s%\shilfreich · 7 Bewertungen$/)).toBeTruthy();
    expect(within(voll).getByText("14 von 17 Vorschlägen übernommen")).toBeTruthy();
    expect(within(voll).getByText("14 übernommen · 2 angepasst · 1 verworfen")).toBeTruthy();

    // Below five verdicts the count alone; with none, words — never 0 %.
    const wenig = record(await karte("Rechnungseinwände"));
    expect(within(wenig).getByText("2 Bewertungen")).toBeTruthy();
    expect(within(wenig).getByText("3 Vorschläge entschieden")).toBeTruthy();
    expect(wenig.textContent).not.toMatch(/%/);
    const keine = record(await karte("EEG-Anlagen"));
    expect(within(keine).getByText("Noch keine Bewertungen")).toBeTruthy();
    expect(within(keine).getByText("Noch kein Vorschlag entschieden")).toBeTruthy();
    expect(keine.textContent).not.toMatch(/%/);
  });

  it("shows the assistant's record as an Überblick tile that opens the Beipackzettel", async () => {
    let bilanz: Record<string, unknown> = BILANZ;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) =>
        url === "/svc/agentd/api/v1/agents/catalog"
          ? Response.json({ faehigkeiten: [assistentImKatalog(bilanz)] })
          : new Response("{}", { status: 503 }),
      ),
    );
    await open("/ueberblick");
    const tile = await screen.findByRole("link", { name: /^Assistent: 86\s%/ });
    expect(tile.getAttribute("href")).toBe("/assistent");
    expect(tile.textContent).toContain("hilfreich · 7 Bewertungen in 30 Tagen");
    expect(tile.textContent).toContain("14 von 17 Vorschlägen übernommen");
    cleanup();

    bilanz = { ...BILANZ, hilfreich: { ja: 2, nein: 0 }, vorschlag: {} };
    await open("/ueberblick");
    const wenig = await screen.findByRole("link", {
      name: /^Assistent: 2 Bewertungen in 30 Tagen/,
    });
    expect(wenig.textContent).not.toMatch(/%/);
    expect(wenig.textContent).toContain("Noch kein Vorschlag entschieden");
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
    // Every row of the table, label beside its keys — a key may serve two groups.
    for (const s of SHORTCUTS) {
      const term = within(sheet)
        .getAllByText(s.label)
        .find((el) => el.tagName === "DT");
      expect(term?.parentElement?.textContent).toContain(s.keys);
    }
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
