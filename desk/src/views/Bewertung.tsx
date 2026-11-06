import { useState } from "react";
import { Button as AriaButton, Input, TextField } from "react-aria-components";
import { useHold } from "../app/Holds";
import { AgentAbgelehnt, bewerten } from "../data/agent";
import { useVerweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";

const TOGGLE =
  "focus-ring inline-flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-caption " +
  "data-[hovered]:bg-fill disabled:opacity-50";

type Stand = "offen" | "gehalten" | "gespeichert" | { fehler: string };

/**
 * „Hilfreich?" under an answer or a Hinweis — the person's verdict, kept by
 * agentd beside the run so a skill's quality is read from the journal. After
 * „Nein", one optional line says what was missing; nothing else is asked.
 *
 * A verdict is a routine decision (FR-029): it shows at once and leaves the
 * browser when its five-second hold ends, so „Rückgängig" sends nothing.
 */
export function Bewertung({ run, initial }: { run: string; initial?: boolean | null }) {
  const [urteil, setUrteil] = useState<boolean | null>(initial ?? null);
  const [warum, setWarum] = useState("");
  const [begruendet, setBegruendet] = useState(false);
  const [stand, setStand] = useState<Stand>("offen");
  const hold = useHold();
  const verweigert = useVerweigert({ dienst: "agentd", aktion: "bewerten" });

  const halten = (hilfreich: boolean, begruendung?: string) => {
    // What the screen returns to on „Rückgängig".
    const vorher = { urteil, begruendet };
    setUrteil(hilfreich);
    setBegruendet(Boolean(begruendung));
    setStand("gehalten");
    hold(
      `bewertung/${run}`,
      begruendung
        ? "Begründung ergänzt"
        : hilfreich
          ? "Als hilfreich bewertet"
          : "Als nicht hilfreich bewertet",
      (keepalive) =>
        bewerten(run, hilfreich, begruendung, keepalive).then(
          () => setStand("gespeichert"),
          (e: unknown) =>
            setStand({
              fehler:
                e instanceof AgentAbgelehnt
                  ? e.message
                  : "Nicht gespeichert — der Assistent ist nicht erreichbar.",
            }),
        ),
      () => {
        setUrteil(vorher.urteil);
        setBegruendet(vorher.begruendet);
        setStand("offen");
      },
    );
  };

  if (verweigert) {
    return (
      <p className="m-0 text-caption text-text-2">Bewerten ist Ihnen nicht erlaubt: {verweigert}</p>
    );
  }

  const toggle = (an: boolean) =>
    `${TOGGLE} ${an ? "border-ai bg-[color-mix(in_srgb,var(--ai)_12%,transparent)] text-ai" : "border-ai-border text-text-2"}`;

  return (
    <fieldset className="m-0 flex flex-wrap items-center gap-2 border-0 p-0">
      <legend className="float-left mr-1 p-0 text-caption text-text-2">Hilfreich?</legend>
      <AriaButton
        aria-pressed={urteil === true}
        onPress={() => urteil !== true && halten(true)}
        className={toggle(urteil === true)}
      >
        <Icon name="thumb" size={14} /> Ja
      </AriaButton>
      <AriaButton
        aria-pressed={urteil === false}
        onPress={() => urteil !== false && halten(false)}
        className={toggle(urteil === false)}
      >
        <Icon name="thumb" size={14} className="rotate-180" /> Nein
      </AriaButton>
      {urteil === false && !begruendet && (
        <form
          className="flex min-w-48 flex-1"
          onSubmit={(e) => {
            e.preventDefault();
            if (warum.trim()) halten(false, warum.trim());
          }}
        >
          <TextField aria-label="Was fehlte?" value={warum} onChange={setWarum} className="flex-1">
            <Input
              placeholder="Was fehlte? (optional, ↵ sendet)"
              maxLength={500}
              className="focus-ring h-7 w-full rounded-full border border-ai-border bg-surface px-3 text-caption text-text outline-none"
            />
          </TextField>
        </form>
      )}
      {stand === "gespeichert" && (urteil === true || begruendet) && (
        <span className="text-caption text-text-2">Danke — das bleibt beim Lauf.</span>
      )}
      {typeof stand === "object" && (
        <span className="text-caption text-critical">{stand.fehler}</span>
      )}
    </fieldset>
  );
}
