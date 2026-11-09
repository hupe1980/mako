// Business dates are German calendar dates and Fristen run in gesetzlicher
// deutscher Zeit, so every instant renders in Europe/Berlin whatever zone the
// browser runs in, and a business date never passes through a JS `Date` — a
// `Date` built from "2026-01-01" is midnight UTC, which is 31.12.2025 in New York.

//
// The Sprache picks the style — `03.11.2026, 14:05` in German, `03/11/2026,
// 14:05` in English — and never the zone: an English screen still shows the
// Berlin instant, because the Frist is a German one.

import { type Sprache, sprache } from "../i18n";
import type { BusinessDate, Instant } from "./brands";
import { perSprache } from "./locale";

const BERLIN = "Europe/Berlin";

const berlin = perSprache(
  (locale) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: BERLIN,
      day: "2-digit",
      month: "2-digit",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }),
);

const berlinTime = perSprache(
  (locale) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: BERLIN,
      hour: "2-digit",
      minute: "2-digit",
    }),
);

const utc = perSprache(
  (locale) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: "UTC",
      day: "2-digit",
      month: "2-digit",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }),
);

/** `03.11.2026, 14:05` — the instant in Berlin. */
export function formatInstant(at: Instant): string {
  return berlin().format(new Date(at));
}

/** `14:05` — the instant's Berlin wall-clock time. */
export function formatBerlinTime(at: Instant): string {
  return berlinTime().format(new Date(at));
}

/** `03.11.2026, 13:05:00 UTC` — the same instant as the wire carries it. */
export function formatInstantUtc(at: Instant): string {
  return `${utc().format(new Date(at))} UTC`;
}

const TAG_TRENNER: Record<Sprache, string> = { de: ".", en: "/" };

/** `01.01.2026` (English `01/01/2026`) from `2026-01-01`, by rearranging the digits. */
export function formatBusinessDate(day: BusinessDate): string {
  const [y, m, d] = day.split("-");
  const t = TAG_TRENNER[sprache()];
  return `${d}${t}${m}${t}${y}`;
}

/** Milliseconds since the epoch for an instant — for ordering and countdowns only. */
export function epochMs(at: Instant): number {
  return Date.parse(at);
}

const berlinYmd = new Intl.DateTimeFormat("en-CA", {
  timeZone: BERLIN,
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

/**
 * The Berlin calendar day an epoch instant falls on, as a day number — for
 * asking whether two instants share a day, never for display.
 */
export function berlinDayNumber(ms: number): number {
  const [y = 0, m = 1, d = 1] = berlinYmd.format(ms).split("-").map(Number);
  return Date.UTC(y, m - 1, d) / 86_400_000;
}
