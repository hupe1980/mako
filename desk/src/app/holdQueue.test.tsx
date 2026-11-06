import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HoldProvider } from "./Holds";
import { HOLD_MS, HoldQueue } from "./holdQueue";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const sender = () => vi.fn(async (_keepalive: boolean) => undefined);

describe("a held decision", () => {
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

  it("is sent once when the hold ends", () => {
    const q = new HoldQueue();
    const send = sender();
    q.hold("a", "Verworfen", send, () => undefined);
    vi.advanceTimersByTime(HOLD_MS - 1);
    expect(send).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    vi.advanceTimersByTime(HOLD_MS * 3);
    expect(send).toHaveBeenCalledOnce();
    expect(send).toHaveBeenCalledWith(false);
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
    expect(a).toHaveBeenCalledWith(true);
    expect(b).toHaveBeenCalledWith(true);
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
