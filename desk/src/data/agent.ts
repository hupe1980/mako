// agentd's run API: a person's question starts a run (`POST /api/v1/runs`,
// answered `202` with the run ids at once) and its answer is read when the
// run has concluded (`GET /api/v1/runs/{run}/antwort`). REST, not MCP: a run is
// started, not a tool read. The shapes are agentd's `Accepted` and `Answer`.

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

/** agentd did not answer, or refused the question. */
export class AgentUnavailable extends Error {
  override name = "AgentUnavailable";
}

/** Ask the Desk's assistant; resolves to the run id. */
export async function fragen(frage: string, anker: Anker): Promise<string> {
  const res = await fetch(BASE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      agent: "desk-assistant",
      event_type: "manual",
      input: { frage, ...anker },
    }),
  }).catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (res.status !== 202) throw new AgentUnavailable(`agentd: ${res.status}`);
  const admitted = (await res.json()) as { run_id: string; refused?: string }[];
  const run = admitted[0];
  if (!run || run.refused) throw new AgentUnavailable(run?.refused ?? "kein Lauf zugelassen");
  return run.run_id;
}

/** The answer of a run so far: `running` until it concludes. */
export async function antwort(run: string): Promise<Answer> {
  const res = await fetch(`${BASE}/${encodeURIComponent(run)}/antwort`).catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (!res.ok) throw new AgentUnavailable(`agentd: ${res.status}`);
  return (await res.json()) as Answer;
}

/**
 * Say whether a run's answer or Hinweis helped. agentd records it under the
 * caller's own name as a sealed verdict beside the run; a later one by the
 * same person replaces it, and `begruendung` is erased with the run's data.
 */
export async function bewerten(
  run: string,
  hilfreich: boolean,
  begruendung?: string,
): Promise<void> {
  const res = await fetch(`${BASE}/${encodeURIComponent(run)}/bewertung`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(begruendung ? { hilfreich, begruendung } : { hilfreich }),
  }).catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (res.status !== 204) throw new AgentUnavailable(`agentd: ${res.status}`);
}

/** Record what became of a run's proposal. */
export async function schicksal(run: string, vorschlag: Schicksal): Promise<void> {
  const res = await fetch(`${BASE}/${encodeURIComponent(run)}/bewertung`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ vorschlag }),
  }).catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (res.status !== 204) throw new AgentUnavailable(`agentd: ${res.status}`);
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
  const res = await fetch("/svc/agentd/api/v1/agents/catalog").catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (!res.ok) throw new AgentUnavailable(`agentd: ${res.status}`);
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
  const res = await fetch("/svc/agentd/api/v1/hinweise").catch((e: unknown) => {
    throw new AgentUnavailable(String(e));
  });
  if (!res.ok) throw new AgentUnavailable(`agentd: ${res.status}`);
  return (await res.json()) as Hinweis[];
}
