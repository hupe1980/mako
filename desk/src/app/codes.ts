// Operator labels for service codes come from the active catalogue.

import { eintrag, katalog } from "../i18n";

export { zustand } from "./prozesse";

/** processd's process family (`gpke`, `geli-gas`, …). */
export const family = (code: string): string => eintrag(katalog().values.family, code);

/** A Sparte as a service spells it (`STROM`, `GAS`). */
export const sparte = (code: string): string => eintrag(katalog().values.sparte, code);

/** edmd's reading-order state. */
export const readingStatus = (code: string): string =>
  eintrag(katalog().values.readingStatus, code);

/** Why edmd owes a reading. */
export const readingReason = (code: string): string =>
  eintrag(katalog().values.readingReason, code);

/** Why billingd has not billed a period, by its `reason_code`. */
export const abrechnungsGrund = (code: string): string =>
  eintrag(katalog().values.abrechnungsGrund, code);

/** What a Klärfall asks for, by processd's `aktion`. */
export function klaerfallAktion(code: string): string {
  const t = katalog();
  return eintrag(t.values.klaerfallAktion, code, t.klaerfall.aktionUnbekannt);
}

/** Where a Klärfall came from, by processd's `origins`. */
export const klaerfallHerkunft = (code: string): string =>
  eintrag(katalog().values.klaerfallHerkunft, code);

/** How an assistant's run ended, as agentd states it. */
export const runStatus = (code: string): string => eintrag(katalog().values.run, code);

/**
 * A refusal a tool wrote as `code: sentence` — the stable code, and the
 * service's own text for „Details". Text with no code prefix has none.
 */
export function refusalCode(text: string): { code: string | null; sentence: string | null } {
  const m = /^([a-z][a-z0-9_]*): /.exec(text);
  const code = m?.[1] ?? null;
  return {
    code,
    sentence: code && Object.hasOwn(katalog().codes, code) ? (katalog().codes[code] ?? null) : null,
  };
}
