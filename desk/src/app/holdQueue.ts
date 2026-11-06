// A routine decision waits in the browser before it leaves (FR-029): the
// person sees it take effect at once, has five seconds to take it back, and no
// service needs a reverse action, because nothing was sent. An action that
// emits a market message never goes through here — it asks for confirmation
// and is sent at once.

/** How long a routine decision waits before it is sent. */
export const HOLD_MS = 5_000;

export interface Hold {
  readonly id: number;
  /** What the toast says was decided: „Vorschlag verworfen". */
  readonly label: string;
}

interface Entry extends Hold {
  /** One decision per key: a later hold on the same key replaces this one. */
  readonly key: string;
  readonly send: (keepalive: boolean) => Promise<unknown>;
  readonly undo: () => void;
  readonly timer: ReturnType<typeof setTimeout>;
}

/**
 * The holds of one page, oldest first. `hold` schedules a send, `undo` drops
 * the newest (or a named) hold without sending it, and `flush` sends every
 * hold at once with `keepalive` — what `pagehide` calls, so a decision made
 * just before the tab closes still arrives.
 */
export class HoldQueue {
  private entries: Entry[] = [];
  private nextId = 1;
  private readonly listeners = new Set<() => void>();
  /** How long each hold lasts. */
  readonly ms: number;

  constructor(ms = HOLD_MS) {
    this.ms = ms;
  }

  /**
   * Hold a decision. A pending hold with the same `key` is superseded: it is
   * dropped unsent, since the later decision replaces it at the service too.
   */
  hold(key: string, label: string, send: Entry["send"], undo: () => void): number {
    const superseded = this.entries.find((e) => e.key === key);
    if (superseded) this.drop(superseded);
    const id = this.nextId++;
    const timer = setTimeout(() => this.release(id, false), this.ms);
    // A new array on every change: it is the snapshot React compares.
    this.entries = [...this.entries, { id, key, label, send, undo, timer }];
    this.emit();
    return id;
  }

  /** Take a hold back — the newest when no id is given. False when none was pending. */
  undo(id?: number): boolean {
    const entry = id === undefined ? this.entries.at(-1) : this.entries.find((e) => e.id === id);
    if (!entry) return false;
    this.drop(entry);
    entry.undo();
    return true;
  }

  /** Send every pending hold now. */
  flush(): void {
    for (const { id } of [...this.entries]) this.release(id, true);
  }

  list(): readonly Hold[] {
    return this.entries;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private release(id: number, keepalive: boolean): void {
    const entry = this.entries.find((e) => e.id === id);
    if (!entry) return;
    this.drop(entry);
    // The caller's `send` reports its own failure in place; the queue only
    // keeps a rejection from going unhandled.
    entry.send(keepalive).catch(() => undefined);
  }

  private drop(entry: Entry): void {
    clearTimeout(entry.timer);
    this.entries = this.entries.filter((e) => e !== entry);
    this.emit();
  }

  private emit(): void {
    for (const l of this.listeners) l();
  }
}
