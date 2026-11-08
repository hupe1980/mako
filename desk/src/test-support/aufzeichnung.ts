// A recorded run: agentd's `GET /api/v1/runs/{run}/schritte` stream, one event
// per line of `desk/fixtures/run-*.jsonl` — `{"event", "id", "data"}` exactly
// as the stream carries them, with an optional `nach_ms` that delays a line
// after the one before it (the budget run in a browser waits it out; the
// unit tests step through the lines by hand). The Desk's stream is tested
// against these without agentd or a model.
//
// Two ways to serve one: `alsSse` writes the text/event-stream body a server
// sends, and `AufgezeichneteQuelle` stands in for the browser's `EventSource`
// in vitest, handing the events to the listeners agentd's would reach.

/** One line of a recorded run. */
export interface Aufzeichnung {
  event: "schritt" | "antwort" | "abgebrochen";
  /** The journal sequence, the SSE `id` a reconnect resumes after. */
  id: string;
  data: unknown;
  /** Milliseconds after the line before; absent is at once. */
  nach_ms?: number;
}

/** The lines of a `run-*.jsonl` fixture; a blank line is skipped, a malformed one refused. */
export function lesen(jsonl: string): Aufzeichnung[] {
  return jsonl
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l, i) => {
      const a = JSON.parse(l) as Aufzeichnung;
      if (!["schritt", "antwort", "abgebrochen"].includes(a.event) || typeof a.id !== "string") {
        throw new Error(`Zeile ${i + 1}: kein Ereignis des Streams: ${l}`);
      }
      return a;
    });
}

/** One event as the stream writes it. */
export function alsSseEreignis(a: Aufzeichnung): string {
  return `event: ${a.event}\nid: ${a.id}\ndata: ${JSON.stringify(a.data)}\n\n`;
}

/**
 * The events after `nach` (a `Last-Event-ID`; absent is from the start), as
 * the stream body — a reconnect resumes behind the last event it saw.
 */
export function nachId(lines: Aufzeichnung[], nach?: string | null): Aufzeichnung[] {
  if (!nach) return lines;
  const i = lines.findIndex((l) => l.id === nach);
  return i < 0 ? lines : lines.slice(i + 1);
}

/** The whole stream body a server sends for a recorded run. */
export function alsSse(lines: Aufzeichnung[], nach?: string | null): string {
  return nachId(lines, nach).map(alsSseEreignis).join("");
}

type Listener = (e: MessageEvent) => void;

/**
 * A stand-in for the browser's `EventSource`: it opens on construction, and
 * the test hands it the recorded lines one at a time (`weiter`) or all at
 * once (`alle`). Every instance a test opened is in `offen`, so it can find
 * the stream a view opened by its URL.
 */
export class AufgezeichneteQuelle {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSED = 2;

  /** Every stream opened since the last `installieren`. */
  static offen: AufgezeichneteQuelle[] = [];
  /** What each URL plays, set by `installieren`. */
  static aufnahmen = new Map<string, Aufzeichnung[]>();

  readonly url: string;
  readyState = 1;
  onerror: ((e: Event) => void) | null = null;
  onopen: ((e: Event) => void) | null = null;
  readonly withCredentials = false;
  private listeners = new Map<string, Set<Listener>>();
  private rest: Aufzeichnung[];

  constructor(url: string | URL) {
    this.url = String(url);
    this.rest = [...(AufgezeichneteQuelle.aufnahmen.get(this.url) ?? [])];
    AufgezeichneteQuelle.offen.push(this);
  }

  addEventListener(type: string, fn: Listener): void {
    const set = this.listeners.get(type) ?? new Set();
    set.add(fn);
    this.listeners.set(type, set);
  }

  removeEventListener(type: string, fn: Listener): void {
    this.listeners.get(type)?.delete(fn);
  }

  close(): void {
    this.readyState = 2;
  }

  /** Lines not yet handed over. */
  get uebrig(): number {
    return this.rest.length;
  }

  /** Hand over the next `n` lines, as the stream would deliver them. */
  weiter(n = 1): void {
    for (let i = 0; i < n; i++) {
      const next = this.rest.shift();
      if (!next) return;
      this.senden(next);
    }
  }

  /** Hand over every line left. */
  alle(): void {
    this.weiter(this.rest.length);
  }

  /** Deliver one event, recorded or not — a repeat after a reconnect, say. */
  senden(a: Aufzeichnung): void {
    if (this.readyState === 2) return;
    const e = new MessageEvent(a.event, { data: JSON.stringify(a.data), lastEventId: a.id });
    for (const fn of this.listeners.get(a.event) ?? []) fn(e);
  }

  /** The connection failed; `endgueltig` is the browser giving up (a 4xx, say). */
  fehler(endgueltig: boolean): void {
    if (endgueltig) this.readyState = 2;
    else this.readyState = 0;
    this.onerror?.(new Event("error"));
  }
}

/**
 * Serve recorded runs at their stream URLs in place of the browser's
 * `EventSource` — `{ "r-1": lines }` serves `/svc/agentd/api/v1/runs/r-1/schritte`.
 * The caller restores the global (`vi.unstubAllGlobals`).
 */
export function installieren(
  runs: Record<string, Aufzeichnung[]>,
  stub: (name: string, value: unknown) => void,
): typeof AufgezeichneteQuelle {
  AufgezeichneteQuelle.offen = [];
  AufgezeichneteQuelle.aufnahmen = new Map(
    Object.entries(runs).map(([run, lines]) => [
      `/svc/agentd/api/v1/runs/${encodeURIComponent(run)}/schritte`,
      lines,
    ]),
  );
  stub("EventSource", AufgezeichneteQuelle);
  return AufgezeichneteQuelle;
}

/** The stream a view opened for `run`. */
export function stromVon(run: string): AufgezeichneteQuelle | undefined {
  return AufgezeichneteQuelle.offen.find((q) =>
    q.url.endsWith(`/runs/${encodeURIComponent(run)}/schritte`),
  );
}
