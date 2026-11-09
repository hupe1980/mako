// Where a finding's `referenz` leads. A service names a setting's entry in
// the operator manual as the docs site's own link (`@/docs/services/makod.md
// #policy-as4-unbekannte-absender`); the Desk opens that page under the
// manual's address from its runtime configuration (`laufzeit.ts`), never a
// page it made up.

/** The page under `basis` a `@/docs/….md#anchor` reference names; `null` for any other shape. */
export function handbuchLink(referenz: string, basis: string): string | null {
  const m = /^@\/(docs\/[\w./-]+?)(?:\/_index)?\.md(#[\w-]+)?$/.exec(referenz);
  if (!m) return null;
  return `${basis}/${m[1]}/${m[2] ?? ""}`;
}
