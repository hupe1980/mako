import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Drift } from "../app/Drift";
import { abdruck, type Dokument } from "./api/dokument";
import { abweichungen, fingerabdruecke } from "./oberflaeche";

// The drift banner: each service's served tools against the snapshot the
// Desk's types were generated from. A typed read against a changed tool fails
// later and obscurely; the banner says so up front, naming the service.

const snapshot = (svc: string) =>
  JSON.parse(
    readFileSync(
      resolve(import.meta.dirname, `../../../services/${svc}/mcp.snapshot.json`),
      "utf8",
    ),
  ) as { name: string; inputSchema: unknown; outputSchema: unknown }[];

/** Each service serves `tools`, or — for the services named in `stumm` — no list at all. */
function served(tools: (svc: string) => unknown[] | null, dokument: Dokument = MARKTD) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.endsWith("/api/v1/openapi.json")) return Response.json(dokument);
      const m = JSON.parse(String(init?.body ?? "{}"));
      const svc = url.split("/")[2] ?? "";
      const list = tools(svc);
      const result = m.method === "tools/list" ? (list ? { tools: list } : {}) : {};
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }), {
        headers: { "content-type": "application/json", "mcp-session-id": "s" },
      });
    }),
  );
}

function show() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Drift />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// The REST documents the Desk was generated against, in this test: marktd's.
const MARKTD: Dokument = { openapi: "3.1.0", info: { title: "marktd", version: "1.0" }, paths: {} };
vi.mock("./api/stand", async () => {
  const { abdruck: a } = await import("./api/dokument");
  const d = { openapi: "3.1.0", info: { title: "marktd", version: "1.0" }, paths: {} };
  return { STAND: { marktd: { version: "1.0", abdruck: a(d) } } };
});

describe("the MCP surface fingerprint", () => {
  it("does not depend on key order or tool order", () => {
    const tools = snapshot("processd");
    const shuffled = [...tools]
      .reverse()
      .map((t) => Object.fromEntries(Object.entries(t).reverse())) as typeof tools;
    expect(abweichungen(shuffled, fingerabdruecke(tools))).toEqual([]);
  });

  it("is no drift when a tool is not listed: a grant may leave it out", () => {
    const tools = snapshot("processd");
    expect(
      abweichungen(
        tools.filter((t) => t.name !== "get_process"),
        fingerabdruecke(tools),
      ),
    ).toEqual([]);
  });

  it("changes when a typed tool's output changes", () => {
    const tools = snapshot("processd");
    const changed = tools.map((t) =>
      t.name === "get_process"
        ? { ...t, outputSchema: { ...(t.outputSchema as object), required: ["nothing"] } }
        : t,
    );
    expect(abweichungen(changed, fingerabdruecke(tools))).toEqual(["get_process"]);
  });
});

describe("the drift banner", () => {
  it("stays away while every service serves its snapshot", async () => {
    const calls: string[] = [];
    served((svc) => {
      calls.push(svc);
      return snapshot(svc);
    });
    show();
    await waitFor(() => expect(calls.length).toBeGreaterThanOrEqual(9));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("names the service whose served tools differ from the snapshot", async () => {
    served((svc) =>
      svc === "processd"
        ? snapshot(svc).map((t) => (t.name === "get_process" ? { ...t, outputSchema: {} } : t))
        : snapshot(svc),
    );
    show();
    const banner = await screen.findByRole("alert");
    expect(banner.textContent).toContain("Prozessüberwachung");
    expect(banner.textContent).not.toContain("processd");
  });

  it("says nothing about a service that does not list its tools", async () => {
    const calls: string[] = [];
    served((svc) => {
      calls.push(svc);
      return null;
    });
    show();
    await waitFor(() => expect(calls.length).toBeGreaterThanOrEqual(9));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("names a service whose served REST document is another version", async () => {
    served((svc) => snapshot(svc), { ...MARKTD, info: { title: "marktd", version: "1.1" } });
    show();
    const banner = await screen.findByRole("alert");
    expect(banner.textContent).toContain("Marktstammdaten");
  });

  it("says nothing while the served REST document is the generated one", async () => {
    const calls: string[] = [];
    served((svc) => {
      calls.push(svc);
      return snapshot(svc);
    });
    show();
    await waitFor(() => expect(calls.length).toBeGreaterThanOrEqual(9));
    expect(abdruck(MARKTD)).toMatch(/^[0-9a-f]{8}$/);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
