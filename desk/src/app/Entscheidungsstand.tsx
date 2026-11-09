import { HaltRing } from "../design/HaltRing";
import { katalog, useT } from "../i18n";
import { type Entscheidbar, schluessel, useFehlschlag } from "./aktionen";
import { nichtErreichbar } from "./dienste";
import { useGehalten } from "./Holds";
import { UNDO } from "./keys";

/** A failure's own words: the service's sentence, or that it did not answer. */
function warum(dienst: Parameters<typeof nichtErreichbar>[0], error: unknown): string {
  if (error instanceof Error && error.name === "SchonEntschieden") {
    const by = (error as Error & { decidedBy?: string | null }).decidedBy;
    const k = katalog().halten;
    return k.schonEntschieden(by ?? k.jemand);
  }
  // Every refusal the decision doors raise carries the service's sentence.
  const eigene = ["Abgelehnt", "SchonErledigt", "ErneutGeparkt", "Verweigert"];
  if (error instanceof Error && eigene.includes(error.name)) return error.message;
  return nichtErreichbar(dienst);
}

/**
 * Where a decision on the entry stands: the hold's ring while it can still be
 * taken back, or — when the service refused it — its words. Where the entry's
 * form is open beside it, the form says the refusal and the ring stays alone.
 */
export function Entscheidungsstand({
  ding,
  fehlschlag: zeigen = true,
}: {
  ding: Entscheidbar;
  /** Say a refusal here; false where the entry's open form says it. */
  fehlschlag?: boolean;
}) {
  const t = useT();
  const key = schluessel(ding);
  const gehalten = useGehalten(key);
  const fehlschlag = useFehlschlag(key);
  if (gehalten)
    return <HaltRing ms={gehalten.ms} label={t.halten.ring(gehalten.hold.label, UNDO.keys)} />;
  if (fehlschlag && zeigen)
    return (
      <span role="status" className="max-w-56 text-caption text-critical">
        {t.halten.nichtGesendet(warum(fehlschlag.dienst, fehlschlag.error))}
      </span>
    );
  return null;
}
