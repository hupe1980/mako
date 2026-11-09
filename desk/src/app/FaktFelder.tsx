// The facts a decision states beside its code — the Bilanzkreis,
// Versorgungsart or Abrechnungsintervall an E/G Bestätigung needs where the
// entry's own did not resolve. processd names each and the values its segment
// admits; the Desk offers exactly those, and a free field only where processd
// lists none.

import { Input, Label, Radio, RadioGroup, TextField } from "react-aria-components";
import type { Fakt } from "../data/gen/processd";

const FELD =
  "focus-ring h-9 rounded-control border border-separator bg-surface px-3 text-body text-text outline-none";

export function FaktFelder({
  fakten,
  werte,
  onChange,
  isDisabled = false,
}: {
  fakten: Fakt[];
  werte: Record<string, string>;
  onChange: (feld: string, wert: string) => void;
  isDisabled?: boolean;
}) {
  return fakten.map((f) =>
    f.werte.length ? (
      <RadioGroup
        key={f.feld}
        value={werte[f.feld] ?? null}
        onChange={(w) => onChange(f.feld, w)}
        isRequired
        isDisabled={isDisabled}
        orientation="horizontal"
        className="flex flex-col gap-1.5"
      >
        <Label className="text-caption text-text-2">{f.bezeichnung}</Label>
        <div className="flex flex-wrap gap-2">
          {f.werte.map((w) => (
            <Radio
              key={w.wert}
              value={w.wert}
              className="focus-ring flex h-9 cursor-default items-center gap-2 rounded-control border border-separator bg-surface px-3 text-body text-text-2 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:text-accent"
            >
              <span className="font-mono font-semibold">{w.wert}</span>
              <span>{w.bedeutung}</span>
            </Radio>
          ))}
        </div>
      </RadioGroup>
    ) : (
      <TextField
        key={f.feld}
        value={werte[f.feld] ?? ""}
        onChange={(w) => onChange(f.feld, w)}
        isRequired
        isDisabled={isDisabled}
        className="flex flex-col gap-1"
      >
        <Label className="text-caption text-text-2">{f.bezeichnung}</Label>
        <Input maxLength={64} spellCheck={false} className={`${FELD} font-mono`} />
      </TextField>
    ),
  );
}

/** Every fact holds a value — one of its listed values where it lists any. */
export function faktenVollstaendig(fakten: Fakt[], werte: Record<string, string>): boolean {
  return fakten.every((f) => {
    const w = werte[f.feld]?.trim() ?? "";
    return f.werte.length ? f.werte.some((x) => x.wert === w) : w.length > 0;
  });
}
