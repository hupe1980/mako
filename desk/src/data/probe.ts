// A rehearsal goes through makod's REST door, `POST /api/v1/probe/{command}`,
// under the person's own token: makod runs the command's door up to its
// pre-send check and answers with the verdict and the message it built.
// Nothing is queued, sent or stored. The answer's type is the one the service
// declares for its `rehearse_command` tool — the same Rust type behind both
// doors — so `npm run gen` holds the wire to it.

import type { ToolOutput } from "./mcp";
import { problem, sentence } from "./problem";
import { verweigern } from "./verweigert";

/** The rehearsal's answer: the verdict and the message the door built. */
export type Probe = ToolOutput<"makod", "rehearse_command">;
export type ProbeNachricht = Probe["nachrichten"][number];
export type ProbeSegment = ProbeNachricht["segmente"][number];
export type ProbeBefund = ProbeSegment["befunde"][number];

/** makod refused to rehearse — an unknown command, one that cannot be rehearsed, an undeclared Rolle. */
export class ProbeAbgelehnt extends Error {
  override name = "ProbeAbgelehnt";
  readonly code: string | undefined;
  constructor(message: string, code: string | undefined) {
    super(message);
    this.code = code;
  }
}

/** Rehearse `command` with `payload`; `marktrolle` only where the command admits several. */
export async function proben(
  command: string,
  payload: Record<string, unknown>,
  marktrolle?: string,
): Promise<Probe> {
  const res = await fetch(`/svc/makod/api/v1/probe/${encodeURIComponent(command)}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(marktrolle ? { marktrolle, payload } : { payload }),
  });
  if (res.ok) return (await res.json()) as Probe;
  const p = await problem(res);
  if (res.status === 403) throw verweigern({ dienst: "makod", aktion: "proben" }, sentence(p));
  throw new ProbeAbgelehnt(sentence(p), p.code);
}
