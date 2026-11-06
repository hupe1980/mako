// agentd's run API: a person's question starts a run (`POST /api/v1/runs`,
// answered `202` with the run ids at once) and its answer is read when the
// run has concluded (`GET /api/v1/runs/{run}/antwort`). REST, not MCP: a run is
// started, not a tool read. The shapes are agentd's `Accepted` and `Answer`.

import { problem, sentence } from "./problem";
import { type Handlung, verweigern } from "./verweigert";

const BASE = "/svc/agentd/api/v1/runs";

/** What a question is about — the Anker on screen. */
export type Anker = { kunde: string } | { malo: string } | { freigabe: string };

export interface AnswerSentence {
  text: string;
  /** The tool call it cites, `server/tool`; null for a sentence with no figure. */
  quelle: string | null;
  /** Every figure in it occurs in the cited call's output. */
  belegt: boolean;
}

export interface Answer {
  run: string;
  /** `running`, or the run's outcome (`succeeded`, `failed`, …). */
  status: string;
  /** agentd's disclosure, shown first. */
  disclosure: string;
  saetze: AnswerSentence[];
  /** A decision the assistant proposes for a Freigabe — prefilled, never sent. */
  vorschlag: Vorschlag | null;
  /** Every service read the run made, in order; `fertig` once it settled. */
  schritte: { quelle: string; fertig: boolean }[];
  /** The reader's own standing verdict — `true` for „hilfreich" — or null. */
  hilfreich: boolean | null;
}

export interface Vorschlag {
  aktion: "freigeben" | "ablehnen";
  /** The Antwortcode a refusal carries; null for an approval. */
  antwortcode: string | null;
  begruendung: string;
  /** The read the Begründung rests on, `server/tool`. */
  quelle: string | null;
  belegt: boolean;
}

/** What became of a proposal, as agentd records it beside the run. */
export type Schicksal = "sent" | "edited" | "dismissed";

/** agentd could not be reached, or failed before answering. */
export class AgentUnavailable extends Error {
  override name = "AgentUnavailable";
}

/** agentd answered and said no — a run it does not admit, a verdict it does not take. */
export class AgentAbgelehnt extends Error {
  override name = "AgentAbgelehnt";
}

async function reach(url: string, init?: RequestInit): Promise<Response> {
  return fetch(url, init).catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
}

/**
 * The error a failed answer is: agentd's own sentence either way, a 403 on an
 * action also recorded so the Desk stops offering it (FR-022).
 */
async function failure(res: Response, handlung?: Handlung): Promise<Error> {
  const why = sentence(await problem(res));
  if (res.status === 403 && handlung) return verweigern(handlung, why);
  return res.status >= 500 ? new AgentUnavailable(why) : new AgentAbgelehnt(why);
}

/** Ask the Desk's assistant; resolves to the run id. */
export async function fragen(frage: string, anker: Anker): Promise<string> {
  const res = await reach(BASE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      agent: "desk-assistant",
      event_type: "manual",
      input: { frage, ...anker },
    }),
  });
  if (res.status !== 202) throw await failure(res, { dienst: "agentd", aktion: "fragen" });
  const admitted = (await res.json()) as { run_id: string; refused?: string }[];
  const run = admitted[0];
  if (!run || run.refused) throw new AgentAbgelehnt(run?.refused ?? "kein Lauf zugelassen");
  return run.run_id;
}

/** The answer of a run so far: `running` until it concludes. */
export async function antwort(run: string): Promise<Answer> {
  const res = await reach(`${BASE}/${encodeURIComponent(run)}/antwort`);
  if (!res.ok) throw await failure(res);
  return (await res.json()) as Answer;
}

/**
 * Say whether a run's answer or Hinweis helped. agentd records it under the
 * caller's own name as a sealed verdict beside the run; a later one by the
 * same person replaces it, and `begruendung` is erased with the run's data.
 */
export function bewerten(
  run: string,
  hilfreich: boolean,
  begruendung?: string,
  keepalive = false,
): Promise<void> {
  return verdict(run, begruendung ? { hilfreich, begruendung } : { hilfreich }, keepalive);
}

/** Record what became of a run's proposal. */
export function schicksal(run: string, vorschlag: Schicksal, keepalive = false): Promise<void> {
  return verdict(run, { vorschlag }, keepalive);
}

/**
 * Both go through one door. `keepalive` lets the request outlive the page —
 * a held decision sent on `pagehide`.
 */
async function verdict(run: string, body: object, keepalive: boolean): Promise<void> {
  const res = await reach(`${BASE}/${encodeURIComponent(run)}/bewertung`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    keepalive,
  });
  if (res.status !== 204) throw await failure(res, { dienst: "agentd", aktion: "bewerten" });
}

/** One skill, as agentd's catalog states it from its manifest. */
export interface Faehigkeit {
  name: string;
  titel: string | null;
  zweck: string | null;
  version: string | null;
  digest: string | null;
  aktiv: boolean;
  /** Event types that start it; empty when only a person or a schedule does. */
  ausloeser: string[];
  /** `provider/model`, or null for a skill that runs no model. */
  modell: string | null;
  liest: { werkzeug: string; wozu: string | null }[];
  nur_lesend: boolean;
  mit_freigabe: number;
}

/** Every skill compiled into agentd. */
export async function katalog(): Promise<Faehigkeit[]> {
  const res = await reach("/svc/agentd/api/v1/agents/catalog");
  if (!res.ok) throw await failure(res);
  return ((await res.json()) as { faehigkeiten: Faehigkeit[] }).faehigkeiten;
}

/** An open worklist row a skill opened, filed under its Anker. */
export interface Hinweis {
  task: string;
  run: string;
  art: string;
  skill: string | null;
  titel: string | null;
  /** The row's own summary; a model-written one opens with the disclosure. */
  text: string;
  prioritaet: string;
  faellig: string | null;
  anker: { art: string; wert: string }[];
}

/** The open rows the person's roles may answer. */
export async function hinweise(): Promise<Hinweis[]> {
  const res = await reach("/svc/agentd/api/v1/hinweise");
  if (!res.ok) throw await failure(res);
  return (await res.json()) as Hinweis[];
}
