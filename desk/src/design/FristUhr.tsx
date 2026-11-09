import { Focusable, Tooltip, TooltipTrigger } from "react-aria-components";
import type { Instant } from "../format/brands";
import { formatInstant, formatInstantUtc } from "../format/date";
import { distance, type Urgency, useNow } from "../format/frist";
import { useT } from "../i18n";
import { Icon } from "./Icon";

const LOOK: Record<Urgency, string> = {
  lapsed: "bg-critical text-on-accent",
  critical: "bg-critical-fill text-critical",
  soon: "bg-soon-fill text-soon",
  calm: "bg-fill text-text-2",
};

/**
 * The Frist-Uhr: how far a deadline is, in words and colour, with the instant
 * in Berlin and UTC on hover or focus. `now` is injectable for the gallery.
 */
export function FristUhr({ due, now }: { due: Instant; now?: number }) {
  const t = useT();
  const ticking = useNow();
  const { urgency, text } = distance(due, now ?? ticking);
  return (
    <TooltipTrigger delay={300}>
      <Focusable>
        <time
          dateTime={due}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: the tooltip with the exact instant must be reachable by keyboard
          tabIndex={0}
          className={`focus-ring inline-flex h-6 items-center gap-1 rounded-full px-2 text-body font-medium whitespace-nowrap ${LOOK[urgency]}`}
        >
          <Icon name={urgency === "lapsed" ? "warn" : "clock"} size={14} />
          <span className="sr-only">{t.fristUhr[urgency]}: </span>
          {text}
        </time>
      </Focusable>
      <Tooltip
        offset={6}
        className="rounded-control bg-surface-raised px-3 py-2 text-body text-text shadow-popover"
      >
        <div className="font-medium">{t.zeit.uhr(formatInstant(due))}</div>
        <div className="text-caption text-text-2">{formatInstantUtc(due)}</div>
      </Tooltip>
    </TooltipTrigger>
  );
}
