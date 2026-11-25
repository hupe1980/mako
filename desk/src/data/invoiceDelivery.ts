import type { Instant } from "../format/brands";
import { epochMs } from "../format/date";
import { type Antwort, lesen, RestAbgelehnt } from "./api/lesen";

const path = "/svc/outputd/api/v1/documents/by-subject/{kind}";
type ChannelEvidence = Antwort<typeof path>["deliveries"][number];

function instant(value: unknown): Instant | null {
  if (value == null) return null;
  if (
    typeof value === "string" &&
    /(?:Z|[+-]\d\d:\d\d)$/.test(value) &&
    Number.isFinite(epochMs(value as Instant))
  )
    return value as Instant;
  throw new Error("Invalid channel evidence timestamp");
}

/** Read only the selected invoice under the gateway's current caller identity. */
export async function invoiceDelivery(
  reference: string,
  signal: AbortSignal,
): Promise<ChannelEvidence[] | "denied"> {
  let body: Antwort<typeof path>;
  try {
    body = await lesen(
      "outputd",
      path,
      { path: { kind: "INVOICE" }, query: { subject_ref: reference } },
      signal,
    );
  } catch (err) {
    if (err instanceof RestAbgelehnt) {
      if (err.status === 401 || err.status === 403) return "denied";
      if (err.status === 404 && err.code === "DOCUMENT_NOT_FOUND") return [];
    }
    throw err;
  }
  if (body?.kind !== "INVOICE" || body.subject_ref !== reference || !Array.isArray(body.deliveries))
    throw new Error("Invalid invoice delivery evidence");
  return body.deliveries.map((r: Partial<ChannelEvidence> | null) => {
    if (typeof r?.channel !== "string" || typeof r.status !== "string")
      throw new Error("Invalid channel evidence");
    return {
      channel: r.channel,
      status: r.status,
      delivered_at: instant(r.delivered_at),
      read_at: instant(r.read_at),
      first_sent_at: instant(r.first_sent_at),
    };
  });
}
