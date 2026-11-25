import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Versand } from "../data/versand";
import { HoldProvider } from "./Holds";
import { HOLD_MS, HoldQueue, unbestaetigt } from "./holdQueue";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const sender = () => vi.fn(async (_v: Versand) => undefined);
const SCHLUESSEL = expect.stringMatching(/^[0-9a-f-]{36}$/);

describe("a held decision", () => {
  it("keeps a closing page's case for a read in its initiating role even if keepalive receives a reply", async () => {
    const q = new HoldQueue();
    q.hold("klaerfall/k-1", "held", sender(), () => {}, "lieferant");
    q.flush();
    await vi.advanceTimersByTimeAsync(0);
    expect(unbestaetigt()).toMatchObject([{ key: "klaerfall/k-1", rolle: "lieferant" }]);
    localStorage.clear();
  });

  it("remembers an uncertain decision whose timer expires before the tab closes", async () => {
    const q = new HoldQueue();
    q.hold(
      "klaerfall/k-1",
      "held",
      async () => {
        throw new TypeError("lost response");
      },
      () => {},
      "lieferant",
    );
    await vi.advanceTimersByTimeAsync(HOLD_MS);
    expect(unbestaetigt().map((entry) => entry.key)).toEqual(["klaerfall/k-1"]);
    q.flush();
    expect(unbestaetigt()).toHaveLength(1);
    localStorage.clear();
  });

  it("sends nothing when it is taken back inside the hold", () => {
    const q = new HoldQueue();
    const send = sender();
    const undo = vi.fn();
    q.hold("a", "Verworfen", send, undo);
    vi.advanceTimersByTime(HOLD_MS - 1);
    expect(q.undo()).toBe(true);
    vi.advanceTimersByTime(HOLD_MS * 2);
    expect(send).not.toHaveBeenCalled();
    expect(undo).toHaveBeenCalledOnce();
    expect(q.list()).toHaveLength(0);
  });

  it("takes a new Idempotency-Key when it is held again after an undo", () => {
    const q = new HoldQueue();
    const first = sender();
    q.hold("a", "Verworfen", first, () => undefined);
    q.undo();
    const second = sender();
    q.hold("a", "Verworfen", second, () => undefined);
    q.flush();
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith({ keepalive: true, schluessel: SCHLUESSEL });
  });

  it("is sent once when the hold ends", () => {
    const q = new HoldQueue();
    const send = sender();
    q.hold("a", "Verworfen", send, () => undefined);
    vi.advanceTimersByTime(HOLD_MS - 1);
    expect(send).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    vi.advanceTimersByTime(HOLD_MS * 3);
    expect(send).toHaveBeenCalledOnce();
    expect(send).toHaveBeenCalledWith({ keepalive: false, schluessel: SCHLUESSEL });
    expect(q.undo()).toBe(false);
  });

  it("is replaced, unsent, by a later decision on the same key", () => {
    const q = new HoldQueue();
    const first = sender();
    const second = sender();
    const firstUndo = vi.fn();
    q.hold("bewertung", "Ja", first, firstUndo);
    q.hold("bewertung", "Nein", second, () => undefined);
    vi.advanceTimersByTime(HOLD_MS);
    expect(first).not.toHaveBeenCalled();
    expect(firstUndo).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledOnce();
  });

  it("stacks, and ⌘Z takes back the newest first", () => {
    const q = new HoldQueue();
    const a = sender();
    const b = sender();
    q.hold("a", "A", a, () => undefined);
    q.hold("b", "B", b, () => undefined);
    q.undo();
    vi.advanceTimersByTime(HOLD_MS);
    expect(a).toHaveBeenCalledOnce();
    expect(b).not.toHaveBeenCalled();
  });
});

describe("the page's holds", () => {
  it("are all sent at once, with keepalive, on pagehide", () => {
    const q = new HoldQueue();
    const a = sender();
    const b = sender();
    render(<HoldProvider queue={q}>{null}</HoldProvider>);
    act(() => {
      q.hold("a", "A verworfen", a, () => undefined);
      q.hold("b", "B verworfen", b, () => undefined);
    });
    expect(screen.getAllByRole("button", { name: /Rückgängig/ })).toHaveLength(2);
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(a).toHaveBeenCalledWith({ keepalive: true, schluessel: SCHLUESSEL });
    expect(b).toHaveBeenCalledWith({ keepalive: true, schluessel: SCHLUESSEL });
    // Each decision has its own key.
    expect(a.mock.calls[0]?.[0].schluessel).not.toBe(b.mock.calls[0]?.[0].schluessel);
    vi.advanceTimersByTime(HOLD_MS * 2);
    expect(a).toHaveBeenCalledOnce();
    expect(b).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: /Rückgängig/ })).toBeNull();
  });

  it("answer ⌘Z by taking back the newest, and leave ⌘Z alone with none", () => {
    const q = new HoldQueue();
    const undo = vi.fn();
    render(<HoldProvider queue={q}>{null}</HoldProvider>);
    act(() => {
      q.hold("a", "A verworfen", sender(), undo);
    });
    const taken = fireEvent.keyDown(window, { key: "z", metaKey: true });
    expect(taken).toBe(false);
    expect(undo).toHaveBeenCalledOnce();
    const free = fireEvent.keyDown(window, { key: "z", metaKey: true });
    expect(free).toBe(true);
  });
});
