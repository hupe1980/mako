// Where a Gegenpartei's record is exported: processd's own route, as the
// gateway forwards it (`desk/deploy/gateway.yaml`). Built here from parts the
// page read, each one encoded, so a link never carries a value as markup.

/** The CSV of one Gegenpartei's record over `von`–`bis`, as processd serves it. */
export function exportLink(mpId: string, rolle: string, von: string, bis: string): string {
  const q = new URLSearchParams({ von, bis, format: "csv" });
  return `/svc/processd/api/v1/counterparties/${encodeURIComponent(mpId)}/${encodeURIComponent(rolle)}?${q}`;
}
