import { afterEach, describe, expect, it, vi } from "vitest";
import { typen } from "../../../scripts/api-typen.ts";
import { ServiceUnavailable } from "../mcp";
import { api, RestAbgelehnt, rest } from "./client";
import { type Dokument, zusammenfuehren } from "./dokument";

// The REST door: the services' documents merged as the gateway routes them,
// typed in openapi-fetch's shape, and a failure split as the MCP door splits
// it — refusal or unreachable, never a silent empty answer.

const marktd: Dokument = {
  openapi: "3.1.0",
  info: { title: "marktd", version: "1.0" },
  paths: {
    "/api/v1/malo/{malo_id}": {
      get: {
        parameters: [{ name: "malo_id", in: "path", required: true, schema: { type: "string" } }],
        responses: {
          "200": {
            description: "",
            content: {
              "application/json": { schema: { $ref: "#/components/schemas/MaloResponse" } },
            },
          },
          "404": { description: "" },
        },
      },
    },
  },
  components: {
    schemas: {
      MaloResponse: {
        type: "object",
        required: ["malo_id", "seit"],
        properties: {
          malo_id: { type: "string" },
          seit: { type: "string", format: "date-time" },
        },
      },
    },
  },
};

afterEach(() => vi.unstubAllGlobals());

describe("the merged API document", () => {
  it("puts every path under its gateway prefix and every schema under its service", () => {
    const { dokument, staende } = zusammenfuehren({ marktd });
    expect(Object.keys(dokument.paths ?? {})).toEqual(["/svc/marktd/api/v1/malo/{malo_id}"]);
    expect(Object.keys(dokument.components?.schemas ?? {})).toEqual(["marktd.MaloResponse"]);
    expect(JSON.stringify(dokument)).toContain('"#/components/schemas/marktd.MaloResponse"');
    expect(staende.marktd?.version).toBe("1.0");
    expect(staende.marktd?.abdruck).toMatch(/^[0-9a-f]{8}$/);
  });

  it("is typed in openapi-fetch's shape, a date-time branded as an Instant", async () => {
    const ts = await typen(zusammenfuehren({ marktd }).dokument);
    expect(ts).toContain('"/svc/marktd/api/v1/malo/{malo_id}"');
    expect(ts).toMatch(/get: \{\s*parameters: \{\s*path: \{\s*malo_id: string;/);
    expect(ts).toContain('"application/json": MarktdMaloResponse');
    expect(ts).toContain("seit: Instant");
    expect(ts).toContain('import type { Instant } from "../../format/brands";');
  });
});

describe("a REST read", () => {
  const lesen = () =>
    rest(
      "marktd",
      (api.GET as unknown as (p: string) => ReturnType<typeof api.GET>)(
        "/svc/marktd/api/v1/malo/x",
      ),
    );

  it("answers the service's data", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ malo_id: "x" })),
    );
    expect(await lesen()).toEqual({ malo_id: "x" });
  });

  it("names a refusal by the service's code", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json(
          { error: "Not Found", code: "malo_not_found", detail: "no MaLo x" },
          { status: 404 },
        ),
      ),
    );
    const e = await lesen().catch((x: unknown) => x);
    expect(e).toBeInstanceOf(RestAbgelehnt);
    expect((e as RestAbgelehnt).code).toBe("malo_not_found");
  });

  it("is unreachable, never empty, when the service is down", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Promise.reject(new TypeError("failed to fetch"))),
    );
    expect(await lesen().catch((x: unknown) => x)).toBeInstanceOf(ServiceUnavailable);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("bad gateway", { status: 502 })),
    );
    expect(await lesen().catch((x: unknown) => x)).toBeInstanceOf(ServiceUnavailable);
  });
});
