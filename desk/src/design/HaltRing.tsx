/**
 * A held decision's countdown beside its entry: the ring empties over the
 * hold, then the decision is sent. Under reduced motion it stands full — the
 * label says the same.
 */
export function HaltRing({ ms, label }: { ms: number; label: string }) {
  return (
    <span
      role="timer"
      aria-label={label}
      title={label}
      className="inline-flex size-6 shrink-0 items-center justify-center text-accent"
    >
      <svg viewBox="0 0 24 24" aria-hidden className="size-5 -rotate-90">
        <circle cx="12" cy="12" r="9" fill="none" stroke="var(--separator)" strokeWidth="2.5" />
        <circle
          cx="12"
          cy="12"
          r="9"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          pathLength={1}
          strokeDasharray="1"
          className="ring-drain"
          // Mounted as the hold starts, so the ring empties over all of it.
          style={{ animationDuration: `${ms}ms` }}
        />
      </svg>
    </span>
  );
}
