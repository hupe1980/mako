import { Link } from "react-aria-components";
import { formatCount } from "../format/count";

/** What a tile shows: a number a service returned, or why there is none. */
export type TileState =
  | { kind: "loading" }
  | { kind: "failed"; service: string }
  | { kind: "count"; n: number; atLeast?: boolean; breach?: boolean };

/**
 * One number, its source, and the list it counts — the whole tile is the link.
 * Colour only where the service marks a breach; the Desk sets no threshold.
 */
export function Tile({
  label,
  source,
  href,
  state,
}: {
  label: string;
  source: string;
  href: string;
  state: TileState;
}) {
  const breach = state.kind === "count" && state.breach;
  const value =
    state.kind === "count"
      ? formatCount(state.n, state.atLeast)
      : state.kind === "failed"
        ? "—"
        : "";
  return (
    <Link
      href={href}
      aria-label={
        state.kind === "count"
          ? `${label}: ${value}`
          : state.kind === "failed"
            ? `${label}: ${state.service} nicht erreichbar`
            : `${label}: lädt`
      }
      className={`squircle focus-ring group flex min-h-36 flex-col rounded-panel border bg-surface p-5 no-underline transition-colors data-[hovered]:bg-fill ${
        breach ? "border-[color-mix(in_srgb,var(--critical)_45%,transparent)]" : "border-separator"
      }`}
    >
      <span className="flex items-center gap-2 text-caption font-semibold uppercase tracking-wide text-text-2">
        {breach && <span aria-hidden="true" className="size-2 rounded-full bg-critical" />}
        {label}
      </span>
      <span className="mt-auto pt-4">
        {state.kind === "loading" ? (
          <span className="skeleton block h-10 w-20" />
        ) : (
          <span
            className={`block text-[40px] leading-none font-bold tracking-tight tabular-nums ${
              breach ? "text-critical" : state.kind === "failed" ? "text-text-2" : "text-text"
            }`}
          >
            {value}
          </span>
        )}
      </span>
      <span className="mt-3 flex items-center justify-between text-caption text-text-2">
        <span>{state.kind === "failed" ? `${state.service} nicht erreichbar` : source}</span>
        <span
          aria-hidden="true"
          className="opacity-0 transition-opacity group-data-[hovered]:opacity-100"
        >
          Liste öffnen →
        </span>
      </span>
    </Link>
  );
}
