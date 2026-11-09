import type { ReactNode } from "react";
import { GridListItem } from "react-aria-components";

/**
 * One row of a list the person moves through by keyboard: a title, a quieter
 * line under it, something leading (a monogram) and something trailing (a
 * Frist-Uhr). It is a React Aria `GridListItem`, so it lives in a `GridList`
 * — ↑ ↓ move, ↵ acts, the selected row is marked for sight and for assistive
 * technology alike. `textValue` is what type-ahead and a screen reader read.
 */
export function ListRow({
  id,
  textValue,
  title,
  subtitle,
  leading,
  trailing,
}: {
  id: string;
  textValue: string;
  title: ReactNode;
  subtitle?: ReactNode;
  leading?: ReactNode;
  trailing?: ReactNode;
}) {
  return (
    <GridListItem
      id={id}
      textValue={textValue}
      className="group focus-ring relative flex cursor-default items-center gap-3 border-b border-separator px-4 py-3 outline-none last:border-b-0 data-[hovered]:bg-fill data-[selected]:bg-[color-mix(in_srgb,var(--accent)_12%,transparent)] data-[focus-visible]:-outline-offset-2"
    >
      {/* The selection mark: a bar in the Rolle's accent, so selected is
          never told by colour of the fill alone. */}
      <span
        aria-hidden="true"
        className="absolute inset-y-1.5 left-0 w-[3px] rounded-r-full bg-accent opacity-0 group-data-[selected]:opacity-100"
      />
      {leading}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-callout font-medium text-text">{title}</span>
        {subtitle && (
          <span className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-body text-text-2">
            {subtitle}
          </span>
        )}
      </span>
      {trailing}
    </GridListItem>
  );
}
