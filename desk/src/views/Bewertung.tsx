import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { Button as AriaButton, Input, TextField } from "react-aria-components";
import { bewerten } from "../data/agent";
import { Icon } from "../design/Icon";

const TOGGLE =
  "focus-ring inline-flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-caption " +
  "data-[hovered]:bg-fill disabled:opacity-50";

/**
 * „Hilfreich?" under an answer or a Hinweis — the person's verdict, kept by
 * agentd beside the run so a skill's quality is read from the journal. After
 * „Nein", one optional line says what was missing; nothing else is asked.
 */
export function Bewertung({ run, initial }: { run: string; initial?: boolean | null }) {
  const [urteil, setUrteil] = useState<boolean | null>(initial ?? null);
  const [warum, setWarum] = useState("");
  const [gesendet, setGesendet] = useState(false);
  const senden = useMutation({
    mutationFn: (v: { hilfreich: boolean; begruendung?: string }) =>
      bewerten(run, v.hilfreich, v.begruendung),
  });

  const waehle = (hilfreich: boolean) => {
    setUrteil(hilfreich);
    setGesendet(false);
    senden.mutate({ hilfreich });
  };
  const toggle = (an: boolean) =>
    `${TOGGLE} ${an ? "border-ai bg-[color-mix(in_srgb,var(--ai)_12%,transparent)] text-ai" : "border-ai-border text-text-2"}`;

  return (
    <fieldset className="m-0 flex flex-wrap items-center gap-2 border-0 p-0">
      <legend className="float-left mr-1 p-0 text-caption text-text-2">Hilfreich?</legend>
      <AriaButton
        aria-pressed={urteil === true}
        isDisabled={senden.isPending}
        onPress={() => waehle(true)}
        className={toggle(urteil === true)}
      >
        <Icon name="thumb" size={14} /> Ja
      </AriaButton>
      <AriaButton
        aria-pressed={urteil === false}
        isDisabled={senden.isPending}
        onPress={() => waehle(false)}
        className={toggle(urteil === false)}
      >
        <Icon name="thumb" size={14} className="rotate-180" /> Nein
      </AriaButton>
      {urteil === false && !gesendet && (
        <form
          className="flex min-w-48 flex-1"
          onSubmit={(e) => {
            e.preventDefault();
            if (!warum.trim()) return;
            senden.mutate(
              { hilfreich: false, begruendung: warum.trim() },
              { onSuccess: () => setGesendet(true) },
            );
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
      {senden.isSuccess && (urteil === true || gesendet) && (
        <span className="text-caption text-text-2">Danke — das bleibt beim Lauf.</span>
      )}
      {senden.isError && (
        <span className="text-caption text-critical">
          Nicht gespeichert — der Assistent ist nicht erreichbar.
        </span>
      )}
    </fieldset>
  );
}
