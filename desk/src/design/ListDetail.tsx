import { type KeyboardEvent, type ReactNode, useRef, useState } from "react";
import { Button, GridList } from "react-aria-components";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/**
 * The list–detail–inspector layout: the list on the left, the
 * selected entry in the middle, its Angaben on the right — Mail's shape.
 *
 * Keyboard: ↑ ↓ move the selection and the detail follows; ↵ opens it, moving
 * the focus into the detail; esc from the detail or the Angaben returns the
 * focus to the selected row. The layout answers its container, not the
 * window: three panes when there is room, the Angaben under the detail when
 * there is less, and one pane at a phone's width, where ↵ opens the detail and
 * „Zurück zur Liste“ (or esc) closes it.
 */
export function ListDetail({
  label,
  kopf,
  liste,
  fuss,
  selected,
  onSelect,
  detail,
  angaben,
  hoehe = "h-[calc(100dvh-var(--topbar))]",
}: {
  /** The list's accessible name. */
  label: string;
  /** Title and lead above the list. */
  kopf: ReactNode;
  /** The rows: `ListRow`s, keyed by the ids `selected` names. */
  liste: ReactNode;
  /** Under the list: a service's note that the page is not the whole list. */
  fuss?: ReactNode;
  selected: string | null;
  onSelect: (id: string) => void;
  /** The selected entry; `null` while nothing is selected. */
  detail: ReactNode | null;
  angaben: ReactNode | null;
  /** The layout's height: the view's, under the toolbar, unless embedded. */
  hoehe?: string;
}) {
  const t = useT();
  const listRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLElement>(null);
  // At a phone's width only one pane shows; this says which.
  const [offen, setOffen] = useState(false);
  const zeigt = offen && detail !== null;

  const zurListe = () => {
    setOffen(false);
    // The row the person came from, so the list picks up where they left it.
    requestAnimationFrame(() => {
      const row = selected
        ? listRef.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(selected)}"]`)
        : null;
      (row ?? listRef.current)?.focus();
    });
  };

  const onEsc = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    e.preventDefault();
    zurListe();
  };

  return (
    <div className={`@container min-h-0 ${hoehe}`}>
      <div className="grid h-full min-h-0 grid-cols-1 @3xl:grid-cols-[var(--pane-list)_1fr] @6xl:grid-cols-[var(--pane-list)_1fr_var(--pane-inspector)]">
        <div
          className={`min-h-0 flex-col overflow-auto border-separator @3xl:flex @3xl:border-r ${
            zeigt ? "hidden" : "flex"
          }`}
        >
          <div className="px-5 pt-6 pb-4">{kopf}</div>
          <GridList
            ref={listRef}
            aria-label={label}
            selectionMode="single"
            selectionBehavior="replace"
            disallowEmptySelection
            selectedKeys={selected ? [selected] : []}
            onSelectionChange={(keys) => {
              const [next] = [...keys];
              if (typeof next === "string") onSelect(next);
            }}
            onAction={(key) => {
              if (typeof key === "string") onSelect(key);
              setOffen(true);
              requestAnimationFrame(() => detailRef.current?.focus());
            }}
            className="border-t border-separator outline-none"
          >
            {liste}
          </GridList>
          {fuss}
        </div>

        {/* One scroll for the detail and its Angaben while they share a
            column; with room for a third pane the wrapper dissolves
            (`display: contents`) and each becomes a pane of its own. */}
        {/* biome-ignore lint/a11y/noStaticElementInteractions: it only hears esc bubbling up from the focusable content, to return to the list */}
        <div
          onKeyDown={onEsc}
          className={`min-h-0 flex-col overflow-auto @3xl:flex @6xl:contents ${zeigt ? "flex" : "hidden"}`}
        >
          <section
            ref={detailRef}
            tabIndex={-1}
            aria-label={label}
            className="focus-ring flex shrink-0 flex-col @6xl:min-h-0 @6xl:shrink @6xl:overflow-auto"
          >
            {detail === null ? (
              <div className="m-auto max-w-xs px-6 py-16 text-center">
                <p className="m-0 text-title font-semibold text-text">{t.liste.nichtsGewaehlt}</p>
                <p className="m-0 mt-2 text-callout text-text-2">{t.liste.nichtsGewaehltText}</p>
              </div>
            ) : (
              <>
                <div className="px-5 pt-4 @3xl:hidden">
                  <Button
                    onPress={zurListe}
                    className="focus-ring -ml-2 flex h-8 items-center gap-1 rounded-[8px] px-2 text-body font-medium text-accent data-[hovered]:bg-fill"
                  >
                    <Icon name="chevron" size={14} className="rotate-180" />
                    {t.liste.zurueck}
                  </Button>
                </div>
                {detail}
              </>
            )}
          </section>
          {detail !== null && angaben && (
            <aside
              aria-label={t.liste.angaben}
              className="shrink-0 border-t border-separator px-6 py-5 @6xl:min-h-0 @6xl:overflow-auto @6xl:border-t-0 @6xl:border-l @6xl:bg-sidebar"
            >
              {angaben}
            </aside>
          )}
        </div>
      </div>
    </div>
  );
}

/** One fact in the Angaben: its name above, the value under it. */
export function Angabe({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="border-b border-separator py-2.5 first:pt-0 last:border-b-0">
      <dt className="text-caption font-semibold uppercase tracking-wide text-text-2">{name}</dt>
      <dd className="m-0 mt-1 text-body text-text">{children}</dd>
    </div>
  );
}
