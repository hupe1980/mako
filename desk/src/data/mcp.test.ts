import { afterEach, describe, expect, it, vi } from "vitest";
import { call, ServiceUnavailable, ToolRefusal } from "./mcp";

// What makod's rmcp transport sends back: an event stream whose data line
// carries the JSON-RPC answer, and the session id on the initialize response.
function sse(message: unknown, session?: string): Response {
  const body = `data: \nid: 0\nretry: 3000\n\ndata: ${JSON.stringify(message)}\nid: 0/1\n`;
  return new Response(body, {
    headers: {
      "content-type": "text/event-stream",
      ...(session ? { "mcp-session-id": session } : {}),
    },
  });
}

function server(toolAnswer: (id: number) => Response) {
  return vi.fn(async (_url: string, init: RequestInit) => {
    const message = JSON.parse(String(init.body));
    if (message.method === "initialize")
      return sse({ jsonrpc: "2.0", id: message.id, result: {} }, "s1");
    if (message.method === "notifications/initialized") return new Response(null, { status: 202 });
    return toolAnswer(message.id);
  });
}

afterEach(() => vi.unstubAllGlobals());

describe("the MCP client", () => {
  it("returns a tool's structured content", async () => {
    const view = { business_key: "51238696012", deadlines: [] };
    const fetch = server((id) =>
      sse({ jsonrpc: "2.0", id, result: { content: [], structuredContent: view } }),
    );
    vi.stubGlobal("fetch", fetch);

    await expect(call("makod", "get_process", { business_key: "51238696012" })).resolves.toEqual(
      view,
    );
    const toolCall = fetch.mock.calls.at(-1)?.[1] as RequestInit;
    expect((toolCall.headers as Record<string, string>)["mcp-session-id"]).toBe("s1");
  });

  it("turns a tool error into a refusal in the service's words", async () => {
    vi.stubGlobal(
      "fetch",
      server((id) =>
        sse({
          jsonrpc: "2.0",
          id,
          result: { isError: true, content: [{ type: "text", text: "process_not_found: …" }] },
        }),
      ),
    );
    const failure = call("makod", "get_process", { business_key: "x" });
    await expect(failure).rejects.toBeInstanceOf(ToolRefusal);
    await expect(failure).rejects.toThrow("process_not_found: …");
  });

  it("reports an unreachable service once the retry fails too", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("bad gateway", { status: 502 })),
    );
    await expect(call("makod", "get_process", { business_key: "x" })).rejects.toBeInstanceOf(
      ServiceUnavailable,
    );
  });
});
