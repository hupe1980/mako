// Formats a service schema states map to the Desk's branded strings, so a
// value can only be shown through src/format: date-time → Instant, date →
// BusinessDate, decimal → Decimal. Shared by the MCP (`gen.ts`) and the REST
// (`gen-api.ts`) generators, so both doors' types name the same brands.

export const BRANDS: Record<string, string> = {
  "date-time": "Instant",
  date: "BusinessDate",
  decimal: "Decimal",
};

export type Json = { [key: string]: unknown };

/** Replace a branded `format` with the brand's type, keeping `| null`. */
export function brand(node: unknown): unknown {
  if (Array.isArray(node)) return node.map(brand);
  if (node === null || typeof node !== "object") return node;
  const obj = node as Json;
  const tsType = typeof obj.format === "string" ? BRANDS[obj.format] : undefined;
  if (tsType) {
    const nullable = Array.isArray(obj.type) && obj.type.includes("null");
    const { format: _f, type: _t, ...rest } = obj;
    return { ...rest, tsType: nullable ? `${tsType} | null` : tsType };
  }
  return Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, brand(v)]));
}
