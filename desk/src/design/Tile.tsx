import { Link } from "react-aria-components";
import { formatCount } from "../format/count";
import { useT } from "../i18n";

/** What a tile shows: a number a service returned, or why there is none. */
export type TileState =
  | { kind: "loading" }
  | { kind: "failed"; service: string }
  | { kind: "count"; n: number; atLeast?: boolean; breach?: boolean }
  /**
   * A figure that is not a count — a share, already formatted — with the
   * lines that say what it is of. Without `wert`, the first line stands in its
   * place: there is no figure to show yet, and the tile says so in words.
   */
  | { kind: "wert"; wert?: string; zeilen: string[] };

/**
 * One number, its source, and the list it counts — the whole tile is the link.
 * Colour only where the service marks a breach; the Desk sets no threshold.
 */
export function Tile({
  label,
  source,
  href,
  state,
  ziel,
}: {
  label: string;
  source: string;
  href: string;
  state: TileState;
  /** What the click opens, said on hover; „Liste öffnen“ when not named. */
  ziel?: string;
}) {
  const t = useT();
  const breach = state.kind === "count" && state.breach;
  const value =
    state.kind === "count"
      ? formatCount(state.n, state.atLeast)
      : state.kind === "wert"
        ? (state.wert ?? state.zeilen[0] ?? "")
        : state.kind === "failed"
          ? "—"
          : "";
  const zeilen = state.kind === "wert" ? state.zeilen.slice(state.wert ? 0 : 1) : [];
  return (
    // The link's name is its visible text — label, figure, lines, source — so a
    // person who reads a tile aloud can say what is on it (WCAG 2.5.3).
    <Link
      href={href}
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
          <span className="skeleton block h-10 w-20">
            <span className="sr-only">{t.steuer.laedt}</span>
          </span>
        ) : state.kind === "wert" && !state.wert ? (
          <span className="block text-title font-semibold text-text-2">{value}</span>
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
      {zeilen.length > 0 && (
        <span className="mt-2 flex flex-col text-caption text-text-2">
          {zeilen.map((z) => (
            <span key={z}>{z}</span>
          ))}
        </span>
      )}
      <span className="mt-3 flex items-center justify-between text-caption text-text-2">
        <span>{state.kind === "failed" ? `${state.service} nicht erreichbar` : source}</span>
        <span
          aria-hidden="true"
          className="opacity-0 transition-opacity group-data-[hovered]:opacity-100"
        >
          {ziel ?? t.steuer.listeOeffnen} →
        </span>
      </span>
    </Link>
  );
}
