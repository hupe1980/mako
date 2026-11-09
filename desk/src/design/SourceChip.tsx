import { Button, Dialog, DialogTrigger, Heading, Popover } from "react-aria-components";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/** Where a figure comes from, as the service attributed it. */
export interface Quelle {
  dokument: string;
  kapitel?: string;
  seite?: string;
  regel?: string;
}

// A Fundstelle cites a German document, so its chapter and page keep the
// document's own abbreviations in every Sprache.
function short(q: Quelle): string {
  return [q.dokument, q.kapitel && `Kap. ${q.kapitel}`, q.seite && `S. ${q.seite}`]
    .filter(Boolean)
    .join(" · ");
}

/**
 * The capsule after a figure that names its rule. Only ever rendered from an
 * attribution a service returned — the Desk never invents one.
 */
export function SourceChip({ quelle }: { quelle: Quelle }) {
  const t = useT();
  return (
    <DialogTrigger>
      <Button
        aria-label={t.quelle.knopf(short(quelle))}
        className="focus-ring inline-flex min-h-5 max-w-full items-center gap-1 rounded-[10px] border border-separator px-2 py-px text-left text-caption font-medium text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
      >
        <Icon name="doc" size={12} className="shrink-0" />
        <span className="min-w-0">{short(quelle)}</span>
      </Button>
      <Popover
        offset={6}
        className="w-72 rounded-panel bg-surface-raised p-4 shadow-popover outline-none"
      >
        <Dialog className="outline-none">
          <Heading slot="title" className="m-0 text-caption font-semibold uppercase text-text-2">
            {t.quelle.titel}
          </Heading>
          <p className="m-0 mt-1 text-callout font-medium text-text">{quelle.dokument}</p>
          {(quelle.kapitel || quelle.seite) && (
            <p className="m-0 mt-0.5 text-body text-text-2">
              {[
                quelle.kapitel && `Kapitel ${quelle.kapitel}`,
                quelle.seite && `Seite ${quelle.seite}`,
              ]
                .filter(Boolean)
                .join(", ")}
            </p>
          )}
          {quelle.regel && <p className="m-0 mt-3 text-body text-text">{quelle.regel}</p>}
        </Dialog>
      </Popover>
    </DialogTrigger>
  );
}
