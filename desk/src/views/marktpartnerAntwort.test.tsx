import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RolleProvider } from "../app/rollen";
import { MarktpartnerAntwort } from "./MarktpartnerAntwort";

// The counterparty's answer as processd's read model states it: the code, the
// BDEW's wording from the list the answer named, and that list as its source.

function projection(fields: Record<string, unknown>) {
  return {
    process_id: "p-1",
    pid: 55003,
    family: "gpke",
    workflow_name: "gpke-lf-anmeldung",
    state: "completed",
    malo_id: "51238696012",
    partner_mp_id: null,
    mdm_role: null,
    deadline_at: null,
    deadline_source: null,
    deadline_risk: "unknown",
    started_at: "2026-11-02T09:00:00Z",
    last_event_at: "2026-11-02T10:00:00Z",
    erc_code: null,
    antwort_code: null,
    antwort_codeliste: null,
    bedeutung: null,
    initiator_is_affiliate: false,
    ...fields,
  };
}

/** processd answers `get_process` with `result`; every call is recorded. */
function processd(result: unknown) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit) => {
      const m = JSON.parse(String(init.body));
      if (m.method === "tools/call") calls.push(`${url.split("/")[2]}/${m.params.name}`);
      return new Response(
        JSON.stringify({
          jsonrpc: "2.0",
          id: m.id,
          result: m.method === "tools/call" ? result : {},
        }),
        { headers: { "content-type": "application/json", "mcp-session-id": "s" } },
      );
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
        <MarktpartnerAntwort processId="p-1" />
      </RolleProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the counterparty's answer", () => {
  it("shows the code with processd's wording and the list it came from", async () => {
    const calls = processd({
      structuredContent: projection({
        antwort_code: "A02",
        antwort_codeliste: "E_0622",
        bedeutung: "Wortlaut aus E_0622",
      }),
    });
    show();
    expect(await screen.findByText("A02")).toBeTruthy();
    expect(screen.getByText("Wortlaut aus E_0622")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Quelle: E_0622" })).toBeTruthy();
    expect(calls).toEqual(["processd/get_process"]);
  });

  it("names the list and no meaning when processd has no wording", async () => {
    processd({
      structuredContent: projection({ antwort_code: "Z99", antwort_codeliste: "E_0622" }),
    });
    show();
    expect(await screen.findByText("Z99")).toBeTruthy();
    expect(screen.getByText(/E_0622/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Quelle/ })).toBeNull();
  });

  it("shows nothing for a process nobody has answered", async () => {
    const calls = processd({ structuredContent: projection({}) });
    show();
    await vi.waitFor(() => expect(calls).toHaveLength(1));
    expect(screen.queryByText("Antwort des Marktpartners")).toBeNull();
  });
});
