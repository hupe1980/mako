// Serves the recorded runs in desk/fixtures/ as agentd's event stream, for a
// browser run against Vite (the budget run): the run id `beantwortet` plays
// `fixtures/run-beantwortet.jsonl` at `/svc/agentd/api/v1/runs/beantwortet/schritte`,
// each line after its `nach_ms`, resuming behind a `Last-Event-ID`.
//
//   server.middlewares.use(aufzeichnungen());

import { existsSync, readFileSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { resolve } from "node:path";
import { alsSseEreignis, lesen, nachId } from "../src/test-support/aufzeichnung.ts";

const FIXTURES = resolve(import.meta.dirname, "../fixtures");
const ROUTE = /^\/svc\/agentd\/api\/v1\/runs\/([a-z0-9-]+)\/schritte$/;

/** A Connect middleware: the recorded run's stream, or `next()` for any other path. */
export function aufzeichnungen(dir = FIXTURES) {
  return (req: IncomingMessage, res: ServerResponse, next: () => void) => {
    const run = ROUTE.exec(req.url?.split("?")[0] ?? "")?.[1];
    const file = run && resolve(dir, `run-${run}.jsonl`);
    if (!file || !existsSync(file)) return next();
    const header = req.headers["last-event-id"];
    const lines = nachId(
      lesen(readFileSync(file, "utf8")),
      Array.isArray(header) ? header[0] : header,
    );
    res.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
      connection: "keep-alive",
    });
    let closed = false;
    req.on("close", () => {
      closed = true;
    });
    void (async () => {
      for (const line of lines) {
        if (line.nach_ms) await new Promise((r) => setTimeout(r, line.nach_ms));
        if (closed) return;
        res.write(alsSseEreignis(line));
      }
      res.end();
    })();
  };
}
