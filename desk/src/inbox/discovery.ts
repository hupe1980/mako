import { DIENST } from "../app/dienste";
import { formatCount } from "../format/count";
import { formatBusinessDate, formatInstant } from "../format/date";
import { katalog } from "../i18n";
import type { Stand } from "./model";

export function discoverySummary(d: Stand["discovery"]): string {
  const k = katalog().billingWatch;
  if (!d) return k.never;
  if (d.status === "completed" && d.candidate_count != null)
    return d.candidate_count === 0 ? k.empty : k.found(formatCount(d.candidate_count));
  if (d.status === "running") return k.running;
  switch (d.error_code) {
    case "candidate_source_unavailable":
      return k.sourceFailed;
    case "evidence_write_failed":
      return k.writeFailed;
    case "sweep_interrupted":
      return k.interrupted;
    default:
      return k.unknown;
  }
}

export function discoveryDetails(d: NonNullable<Stand["discovery"]>): string[] {
  const k = katalog().billingWatch;
  return [
    k.source(DIENST.vertragd),
    k.started(formatInstant(d.started_at)),
    ...(d.finished_at ? [k.finished(formatInstant(d.finished_at))] : []),
    k.range(formatBusinessDate(d.ended_from), formatBusinessDate(d.ended_through)),
    ...(d.last_failure_at && d.last_failure_at !== d.finished_at
      ? [k.failedAt(formatInstant(d.last_failure_at))]
      : []),
    d.next_attempt_at ? k.next(formatInstant(d.next_attempt_at)) : k.unscheduled,
  ];
}
