import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
} from "react";
import { Button } from "react-aria-components";
import { Kbd } from "../design/Kbd";
import { type Hold, HoldQueue } from "./holdQueue";
import { UNDO } from "./keys";

const Queue = createContext<HoldQueue | null>(null);

/**
 * The page's held decisions and their toasts. `⌘Z` takes back the newest;
 * `pagehide` sends every hold at once, with `keepalive`, so closing the tab
 * inside the hold does not lose a decision.
 */
export function HoldProvider({ children, queue }: { children: ReactNode; queue?: HoldQueue }) {
  const [q] = useState(() => queue ?? new HoldQueue());
  useEffect(() => {
    const flush = () => q.flush();
    const onKey = (e: KeyboardEvent) => {
      if (UNDO.match?.(e) && q.undo()) e.preventDefault();
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("keydown", onKey);
    };
  }, [q]);
  return (
    <Queue.Provider value={q}>
      {children}
      <HoldToasts queue={q} />
    </Queue.Provider>
  );
}

/**
 * Hold a decision: `send(keepalive)` runs when the hold ends, `undo`
 * restores the screen when the person takes it back. One decision per `key`.
 */
export function useHold() {
  const q = useContext(Queue);
  if (!q) throw new Error("useHold outside HoldProvider");
  return useCallback(
    (
      key: string,
      label: string,
      send: (keepalive: boolean) => Promise<unknown>,
      undo: () => void,
    ) => q.hold(key, label, send, undo),
    [q],
  );
}

/** The hold pending under `key`, with how long a hold lasts; null when none is. */
export function useGehalten(key: string): { hold: Hold; ms: number } | null {
  const q = useContext(Queue);
  const holds = useSyncExternalStore(
    (l) => q?.subscribe(l) ?? (() => undefined),
    () => q?.list() ?? KEINE,
  );
  const hold = holds.find((h) => h.key === key);
  return hold && q ? { hold, ms: q.ms } : null;
}

const KEINE: readonly Hold[] = [];

function HoldToasts({ queue }: { queue: HoldQueue }) {
  const holds = useSyncExternalStore(
    (l) => queue.subscribe(l),
    () => queue.list(),
  );
  return (
    <ol
      aria-live="polite"
      aria-label="Gleich gesendet"
      className="pointer-events-none fixed bottom-5 left-1/2 z-50 m-0 flex -translate-x-1/2 list-none flex-col items-center gap-2 p-0"
    >
      {holds.map((h, i) => (
        <HoldToast key={h.id} hold={h} newest={i === holds.length - 1} queue={queue} />
      ))}
    </ol>
  );
}

function HoldToast({ hold, newest, queue }: { hold: Hold; newest: boolean; queue: HoldQueue }) {
  return (
    <li className="material squircle pointer-events-auto relative flex h-10 min-w-72 items-center gap-3 overflow-hidden rounded-panel border border-separator pr-2 pl-4 shadow-[var(--shadow-popover)]">
      <span className="flex-1 text-body text-text">{hold.label}</span>
      <Button
        onPress={() => queue.undo(hold.id)}
        className="focus-ring flex h-7 items-center gap-1.5 rounded-full px-3 text-body font-semibold text-accent data-[hovered]:bg-fill"
      >
        Rückgängig
        {newest && <Kbd>{UNDO.keys}</Kbd>}
      </Button>
      <span
        aria-hidden
        className="drain absolute bottom-0 left-0 h-0.5 w-full bg-accent"
        // The toast mounts as the hold starts, so the bar drains over all of it.
        style={{ animationDuration: `${queue.ms}ms` }}
      />
    </li>
  );
}
