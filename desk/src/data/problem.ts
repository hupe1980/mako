// Every mako service answers a failed request with one body,
// `mako_service::ApiError`'s: `error` the status's reason phrase, `detail` the
// sentence written for the caller, `code` a stable machine code where the
// service states one, `error_id` on a 500 or an upstream failure — the id the
// service's log line carries, and the only thing of an internal error the body
// holds — plus the fields the service adds (`decided_by` on a lost race).

export interface Problem {
  status: number;
  error?: string;
  detail?: string;
  code?: string;
  error_id?: string;
  [field: string]: unknown;
}

/** The failure body of `res`; a body that is not one is kept as its `detail`. */
export async function problem(res: Response): Promise<Problem> {
  const text = await res.text().catch(() => "");
  try {
    const json: unknown = JSON.parse(text);
    if (json && typeof json === "object" && !Array.isArray(json)) {
      const body = json as Record<string, unknown>;
      // The Abrechnung nests its refusal — `{ "error": { "code", "message" } }`;
      // read as the one body, its sentence is the `detail`.
      const inner = body.error;
      if (inner && typeof inner === "object" && !Array.isArray(inner)) {
        const { message, ...rest } = inner as Record<string, unknown>;
        return {
          ...rest,
          ...(typeof message === "string" ? { detail: message } : {}),
          status: res.status,
        };
      }
      return { ...body, status: res.status };
    }
  } catch {
    // Not JSON — a proxy's page, a plain-text answer: shown as it came.
  }
  return text.trim() ? { status: res.status, detail: text.trim() } : { status: res.status };
}

/**
 * What to show of a failure: the service's own sentence, with the error id to
 * quote to whoever reads the service's log where there is one, else the status.
 */
export function sentence(p: Problem): string {
  const said = p.detail ?? `${p.error ?? "Fehler"} (HTTP ${p.status})`;
  return p.error_id ? `${said} · Fehler-ID ${p.error_id}` : said;
}
