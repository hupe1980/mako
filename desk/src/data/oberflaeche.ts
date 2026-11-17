// The fingerprint of a service's typed MCP surface: for every tool with an
// output schema — the tools the Desk's types are generated from — its input
// and output schema in canonical JSON, hashed. `npm run gen` writes each
// snapshot's fingerprints into src/data/gen; the Desk computes the same over
// what the service serves (`tools/list`), so the two can be compared without
// a version number either side has to remember to bump.

export interface Werkzeug {
  name: string;
  inputSchema?: unknown;
  outputSchema?: unknown;
}

/** JSON with every object's keys sorted, so key order never changes a fingerprint. */
export function kanonisch(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(kanonisch).join(",")}]`;
  if (v !== null && typeof v === "object")
    return `{${Object.keys(v)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${kanonisch((v as Record<string, unknown>)[k])}`)
      .join(",")}}`;
  return JSON.stringify(v) ?? "null";
}

/** FNV-1a, 32 bits, as eight hex digits: a fingerprint, not a defence. */
export function fnv(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

/** Each typed tool's fingerprint, by name; an untyped tool has none. */
export function fingerabdruecke(tools: Werkzeug[]): Record<string, string> {
  return Object.fromEntries(
    tools
      .filter((t) => t.outputSchema !== null && t.outputSchema !== undefined)
      .map((t) => [
        t.name,
        fnv(kanonisch({ inputSchema: t.inputSchema ?? null, outputSchema: t.outputSchema })),
      ]),
  );
}

/**
 * The tools the Desk was generated against that the service now serves
 * differently — another schema, or none. A tool it does not list is not
 * drift: a grant or a role build may leave it out, and the read says so.
 */
export function abweichungen(served: Werkzeug[], generiert: Record<string, string>): string[] {
  const jetzt = fingerabdruecke(served);
  const gelistet = new Set(served.map((t) => t.name));
  return Object.keys(generiert).filter(
    (name) => gelistet.has(name) && jetzt[name] !== generiert[name],
  );
}
