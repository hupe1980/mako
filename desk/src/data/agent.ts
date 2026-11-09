// agentd's run API: a person's question starts a run (`POST /api/v1/runs`,
// answered `202` with the run ids at once), its reads arrive as they are
// journaled (`GET /api/v1/runs/{run}/schritte`, Server-Sent Events), Stop
// cancels it (`POST /api/v1/runs/{run}/stopp`), and its answer is the stream's
// last event — or, where no stream can be read, `GET /api/v1/runs/{run}/antwort`.
// REST, not MCP: a run is started, not a tool read. The shapes are agentd's
// `Accepted`, `Schritt` and `Answer`.

import type { Instant } from "../format/brands";
import { katalog as worte } from "../i18n";
import { problem, sentence } from "./problem";
import { type Handlung, verweigern } from "./verweigert";

const BASE = "/svc/agentd/api/v1/runs";

/** What a question is about — the Anker on screen. */
export type Anker =
  | { kunde: string }
  | { malo: string }
  | { process: string }
  | { freigabe: string }
  | { disput: string }
  | { buchung: string };

/** Every Anker key the desk-assistant's manifest names. */
export type AnkerArt = "kunde" | "malo" | "process" | "freigabe" | "disput" | "buchung";

/** The Anker's key: what it is about. */
export function ankerArt(a: Anker): AnkerArt {
  return (Object.keys(a)[0] ?? "malo") as AnkerArt;
}

/** What „Warum?" asks about a Heute item, with its Anker — in the Sprache in force. */
export function warumFrage(): string {
  return worte().warumFrage.frage;
}

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
  /** A decision the assistant proposes for a Freigabe, a Disput or a geparkte Buchung — prefilled, never sent. */
  vorschlag: Vorschlag | null;
  /** Every service read the run made, in order; `fertig` once it settled. */
  schritte: { quelle: string; fertig: boolean }[];
  /** The reader's own standing verdict — `true` for „hilfreich" — or null. */
  hilfreich: boolean | null;
}

export interface Vorschlag {
  /**
   * For a Freigabe `freigeben` or `ablehnen`; for a Disput `beilegen` or
   * `aufrechterhalten`; for a geparkte Buchung `neu_buchen` or `verwerfen`.
   */
  aktion: "freigeben" | "ablehnen" | "beilegen" | "aufrechterhalten" | "neu_buchen" | "verwerfen";
  /** The Antwortcode a refusal carries; null for an approval and a Disput. */
  antwortcode: string | null;
  /** For a Disput: the checker finding it rests on, as the Rechnungseingang names it. */
  befund?: string | null;
  /** For a Buchung `neu_buchen`: the date to book on, `YYYY-MM-DD`. */
  buchungsdatum?: string | null;
  /** For a Buchung `verwerfen`: why it is dropped. */
  grund?: string | null;
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
 * action also recorded so the Desk stops offering it.
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

/** The read a step made: the service, its tool and what it was asked for. */
export interface SchrittQuelle {
  /** The MCP server, as the grant names it. */
  dienst: string;
  werkzeug: string;
  /** The argument value the step's `objekt` names; null when none does. */
  ref: string | null;
}

/** One read of a run as its journal records it — one line of the trail. */
export interface Schritt {
  /** The journal sequence, also the event's SSE `id`. */
  seq: number;
  /** `lesen`, or `fehlgeschlagen` for a call that did not answer. */
  art: string;
  /** What was done, agentd's verb from the tool's name: „gelesen", „aufgelistet". */
  was: string;
  /** What it was done to, from the call's arguments: „Prozess 4711"; null when none names one. */
  objekt: string | null;
  /** Null exactly when `verborgen`: the person may not read the source. */
  quelle: SchrittQuelle | null;
  verborgen: boolean;
  um: Instant;
}

/** Who stopped a run, and when the journal recorded it. */
export interface Abbruch {
  durch: string;
  um: Instant;
}

/** What a run's stream tells its reader. */
export interface StromLeser {
  schritt: (s: Schritt) => void;
  /** The conclusion — the same `Answer` `GET …/antwort` returns. */
  antwort: (a: Answer) => void;
  abgebrochen: (a: Abbruch) => void;
  /** The stream cannot be read (refused, absent); the answer read is the way left. */
  unlesbar: () => void;
}

/**
 * Follow a run's steps as agentd journals them. The browser's `EventSource`
 * reconnects by itself and sends the last event's id as `Last-Event-ID`, so
 * agentd resumes behind it; a step that still comes twice at the seam is
 * handed over once. The stream is closed after its conclusion, and the
 * returned function closes it early.
 */
export function schritte(run: string, leser: StromLeser): () => void {
  if (typeof EventSource === "undefined") {
    leser.unlesbar();
    return () => undefined;
  }
  const es = new EventSource(`${BASE}/${encodeURIComponent(run)}/schritte`);
  const gesehen = new Set<string>();
  const on = <T>(event: string, fn: (data: T) => void, ende = false) =>
    es.addEventListener(event, (e) => {
      const m = e as MessageEvent<string>;
      if (m.lastEventId && gesehen.has(m.lastEventId)) return;
      if (m.lastEventId) gesehen.add(m.lastEventId);
      if (ende) es.close();
      fn(JSON.parse(m.data) as T);
    });
  on<Schritt>("schritt", leser.schritt);
  on<Answer>("antwort", leser.antwort, true);
  on<Abbruch>("abgebrochen", leser.abgebrochen, true);
  es.onerror = () => {
    // CONNECTING is the browser retrying; CLOSED is it giving up — a refusal
    // or a route agentd does not serve, which only the answer read can name.
    if (es.readyState === es.CLOSED) leser.unlesbar();
  };
  return () => es.close();
}

/**
 * Stop a run the person started. agentd cancels it at its next step boundary
 * and answers `202`; the stream then ends with `abgebrochen`. A run that has
 * concluded already is refused — its answer stands.
 */
export async function stoppen(run: string): Promise<void> {
  const res = await reach(`${BASE}/${encodeURIComponent(run)}/stopp`, { method: "POST" });
  if (res.status !== 202) throw await failure(res, { dienst: "agentd", aktion: "stoppen" });
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

/** What the assistant proposes on unasked: a Freigabe, a Rechnungsdisput, a geparkte Buchung. */
export type VorschlagArt = "freigabe" | "disput" | "buchung";

/** How each proposed action reads, on a row and on its card. */
export const AKTION: Record<Vorschlag["aktion"], string> = {
  freigeben: "Freigeben",
  ablehnen: "Ablehnen",
  beilegen: "Beilegen",
  aufrechterhalten: "Aufrechterhalten",
  neu_buchen: "Neu buchen",
  verwerfen: "Verwerfen",
};

/**
 * The waiting proposal of each entry, by kind and id: its run's answer so far
 * (`running`, or concluded with its checked `vorschlag`), or null where agentd
 * admitted none. Reading starts no run — agentd proposes once per entry.
 */
export async function vorschlaege(
  ids: Partial<Record<VorschlagArt, string[]>>,
): Promise<Partial<Record<VorschlagArt, Record<string, Answer | null>>>> {
  const query = (Object.entries(ids) as [VorschlagArt, string[]][])
    .filter(([, v]) => v.length > 0)
    .map(([art, v]) => `${art}=${v.map(encodeURIComponent).join(",")}`);
  if (query.length === 0) return {};
  const res = await reach(`/svc/agentd/api/v1/vorschlaege?${query.join("&")}`);
  if (!res.ok) throw await failure(res);
  return (await res.json()) as Partial<Record<VorschlagArt, Record<string, Answer | null>>>;
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
  /** Its record over the last `tage` days; null for a skill not activated. */
  bilanz: Bilanz | null;
}

/**
 * What a skill's runs came to over a window — agentplane's track record as
 * agentd's catalog reports it, never a count the Desk keeps.
 */
export interface Bilanz {
  tage: number;
  /** Runs admitted. */
  laeufe: number;
  /** Runs still working. */
  offen: number;
  /** Concluded runs by outcome. */
  abgeschlossen: Record<string, number>;
  /** „Hilfreich?" verdicts. */
  hilfreich: { ja: number; nein: number };
  /** What became of the proposals: `sent` übernommen, `edited` angepasst, `dismissed` verworfen. */
  vorschlag: { sent?: number; edited?: number; dismissed?: number };
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
