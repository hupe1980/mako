import { useQuery } from "@tanstack/react-query";
import { useRolle } from "../app/rollen";
import type { CodeAuswahl, Freigabe } from "../data/gen/processd";
import { call, type ToolOutput } from "../data/mcp";
import { proben } from "../data/probe";

export type Entscheidungsprobe = ToolOutput<"processd", "rehearse_decision">;

/**
 * processd's rehearsal of the decision the form holds — the command, the body
 * and its hash, or the refusal the decision route would answer with. Asked
 * again whenever the choice changes; nothing is sent.
 */
export function useEntscheidungsprobe(
  f: Freigabe,
  gewaehlt: "freigeben" | "ablehnen" | null,
  code: string | null,
  codes: CodeAuswahl | null,
) {
  const { rolle } = useRolle();
  const bereit = gewaehlt !== null && (!codes || code !== null);
  return useQuery({
    queryKey: [rolle, "processd", "rehearse_decision", f.id, gewaehlt, code],
    queryFn: () =>
      call("processd", "rehearse_decision", {
        id: f.id,
        freigeben: gewaehlt === "freigeben",
        ...(codes && code ? { antwort_code: code, antwort_tree: codes.ebd } : {}),
      }),
    enabled: bereit,
  });
}

/**
 * Whether what taking in `vorschlag` would send can be rendered by the
 * services: processd rehearses the proposed decision and makod renders its
 * message. `ja` also for a decision that sends no message; `nein` where either
 * refuses — then the proposal is never taken in unchanged (M84 SR-035).
 */
export function useDarstellbar(
  f: Freigabe | null,
  vorschlag: { aktion: string; antwortcode: string | null } | null,
): "ja" | "nein" | "offen" {
  const { rolle } = useRolle();
  const freigeben = vorschlag?.aktion === "freigeben";
  const codes = freigeben ? (f?.codes_freigeben ?? null) : (f?.codes_ablehnen ?? null);
  const probe = useQuery({
    queryKey: [
      rolle,
      "processd",
      "rehearse_decision",
      f?.id,
      vorschlag?.aktion,
      vorschlag?.antwortcode,
    ],
    queryFn: () =>
      call("processd", "rehearse_decision", {
        id: f?.id ?? "",
        freigeben,
        ...(codes && vorschlag?.antwortcode
          ? { antwort_code: vorschlag.antwortcode, antwort_tree: codes.ebd }
          : {}),
      }),
    enabled:
      vorschlag !== null && (vorschlag.aktion === "freigeben" || vorschlag.aktion === "ablehnen"),
  });
  const p = probe.data;
  const nachricht = useQuery({
    queryKey: [rolle, "makod", "probe", p?.command, p?.body_sha256],
    queryFn: () => proben(p?.command ?? "", p?.payload ?? {}, p?.marktrolle ?? undefined),
    enabled: p?.urteil === "besteht" && Boolean(p.command),
    retry: false,
  });
  if (!vorschlag || !f) return "nein";
  if (probe.isError || p?.urteil === "abgelehnt") return "nein";
  if (p?.urteil === "ohne_nachricht") return "ja";
  if (nachricht.isError) return "nein";
  if (nachricht.data) return "ja";
  return "offen";
}
