// Reads go through the services' MCP tools — the same typed surface an agent is
// granted — over the Desk's same-origin gateway at `/svc/<service>/mcp`.
// Only tools with an output schema are callable: the types come from
// src/data/gen, which `npm run gen` writes from each service's snapshot, and
// the snapshot test in the service is what holds the wire to them.
//
// The Desk speaks the three messages it needs of MCP's streamable HTTP
// transport — `initialize`, `notifications/initialized`, `tools/call` — with
// `fetch`. A response is either JSON or a short event stream whose `data:`
// lines carry the JSON-RPC answer.

import type { BillingdTools } from "./gen/billingd";
import type { EdmdTools } from "./gen/edmd";
import type { InvoicdTools } from "./gen/invoicd";
import type { MakodTools } from "./gen/makod";
import type { MarktdTools } from "./gen/marktd";
import type { NetzbilanzdTools } from "./gen/netzbilanzd";
import type { ProcessdTools } from "./gen/processd";
import type { VertragdTools } from "./gen/vertragd";

/** Every service the Desk reads, with its typed tools. */
export interface ServiceTools {
  billingd: BillingdTools;
  edmd: EdmdTools;
  invoicd: InvoicdTools;
  makod: MakodTools;
  marktd: MarktdTools;
  netzbilanzd: NetzbilanzdTools;
  processd: ProcessdTools;
  vertragd: VertragdTools;
}

export type Service = keyof ServiceTools;
export type ToolName<S extends Service> = keyof ServiceTools[S] & string;
type Tool<S extends Service, T extends ToolName<S>> = ServiceTools[S][T] extends {
  input: infer I;
  output: infer O;
}
  ? { input: I; output: O }
  : never;
export type ToolInput<S extends Service, T extends ToolName<S>> = Tool<S, T>["input"];
export type ToolOutput<S extends Service, T extends ToolName<S>> = Tool<S, T>["output"];

/** The tool answered and said no — not found, not permitted — in its own words. */
export class ToolRefusal extends Error {
  override name = "ToolRefusal";
}

/** The service could not be reached or failed before answering. */
export class ServiceUnavailable extends Error {
  override name = "ServiceUnavailable";
  readonly service: Service;
  constructor(service: Service, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
    this.service = service;
  }
}

const PROTOCOL = "2025-06-18";

interface RpcAnswer {
  id?: number;
  result?: unknown;
  error?: { code: number; message: string };
}

interface CallResult {
  content?: { type: string; text?: string }[];
  structuredContent?: unknown;
  isError?: boolean;
}

let nextId = 1;
const sessions = new Map<Service, Promise<string>>();

async function post(
  service: Service,
  message: Record<string, unknown>,
  session?: string,
): Promise<Response> {
  const response = await fetch(`/svc/${service}/mcp`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      ...(session ? { "mcp-session-id": session, "mcp-protocol-version": PROTOCOL } : {}),
    },
    body: JSON.stringify({ jsonrpc: "2.0", ...message }),
  });
  if (!response.ok) {
    throw new Error(`${service}: HTTP ${response.status} ${await response.text()}`.trim());
  }
  return response;
}

/** The JSON-RPC answer with `id`, from a JSON body or an event stream. */
async function answer(response: Response, id: number): Promise<RpcAnswer> {
  const body = await response.text();
  const candidates = (response.headers.get("content-type") ?? "").includes("event-stream")
    ? body
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice("data:".length).trim())
        .filter((data) => data.startsWith("{"))
    : [body];
  for (const text of candidates) {
    const message = JSON.parse(text) as RpcAnswer;
    if (message.id === id) return message;
  }
  throw new Error(`no answer to request ${id}`);
}

async function request(
  service: Service,
  method: string,
  params: unknown,
  session?: string,
): Promise<{ result: unknown; response: Response }> {
  const id = nextId++;
  const response = await post(service, { id, method, params }, session);
  const message = await answer(response, id);
  if (message.error) throw new Error(`${service}: ${message.error.message}`);
  return { result: message.result, response };
}

function session(service: Service): Promise<string> {
  let open = sessions.get(service);
  if (!open) {
    open = (async () => {
      const { response } = await request(service, "initialize", {
        protocolVersion: PROTOCOL,
        capabilities: {},
        clientInfo: { name: "mako-desk", version: "0" },
      });
      const id = response.headers.get("mcp-session-id");
      if (!id) throw new Error(`${service}: no MCP session id`);
      await post(service, { method: "notifications/initialized" }, id);
      return id;
    })();
    sessions.set(service, open);
    open.catch(() => sessions.delete(service));
  }
  return open;
}

async function once<S extends Service, T extends ToolName<S>>(
  service: S,
  tool: T,
  args: ToolInput<S, T>,
): Promise<ToolOutput<S, T>> {
  const id = await session(service);
  const { result } = await request(service, "tools/call", { name: tool, arguments: args }, id);
  const call = result as CallResult;
  if (call.isError) {
    const text = (call.content ?? []).map((c) => c.text ?? "").join("\n");
    throw new ToolRefusal(text || "Der Dienst hat die Anfrage abgelehnt.");
  }
  if (call.structuredContent == null) {
    throw new Error(`${service}.${tool} answered without structured content`);
  }
  return call.structuredContent as ToolOutput<S, T>;
}

/**
 * Call a typed tool. A failed session (a dev service restarted, its sessions
 * gone) is reopened once; a refusal is the service's answer and is not retried.
 */
export async function call<S extends Service, T extends ToolName<S>>(
  service: S,
  tool: T,
  args: ToolInput<S, T>,
): Promise<ToolOutput<S, T>> {
  try {
    return await once(service, tool, args);
  } catch (first) {
    if (first instanceof ToolRefusal) throw first;
    sessions.delete(service);
    try {
      return await once(service, tool, args);
    } catch (second) {
      if (second instanceof ToolRefusal) throw second;
      throw new ServiceUnavailable(service, second);
    }
  }
}
