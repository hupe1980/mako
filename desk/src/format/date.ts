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

const berlinDay = perSprache(
  (locale) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: BERLIN,
      day: "2-digit",
      month: "2-digit",
      year: "numeric",
    }),
);

/** `03.11.2026` — the Berlin calendar day the instant falls on. */
export function formatInstantDay(at: Instant): string {
  return berlinDay().format(new Date(at));
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

/**
 * Today in Berlin — the end of a view's default window (the Messwerte of the
 * last 30 days). A window to look at, never a Frist: no deadline is derived
 * from it.
 */
export function heuteBerlin(): BusinessDate {
  return berlinYmd.format(Date.now()) as BusinessDate;
}

/** `day` moved by `tage` calendar days, as a business date. */
export function tageVerschoben(day: BusinessDate, tage: number): BusinessDate {
  const [y = 0, m = 1, d = 1] = day.split("-").map((x) => Number.parseInt(x, 10));
  return new Date(Date.UTC(y, m - 1, d + tage)).toISOString().slice(0, 10) as BusinessDate;
}

/** Midnight in Berlin at the start of `day`, as the instant a REST window takes. */
export function berlinMitternacht(day: BusinessDate): Instant {
  const [y = 0, m = 1, d = 1] = day.split("-").map((x) => Number.parseInt(x, 10));
  const utcMittag = Date.UTC(y, m - 1, d, 12);
  const teile = new Intl.DateTimeFormat("en-US", {
    timeZone: BERLIN,
    hour: "2-digit",
    hourCycle: "h23",
  }).format(utcMittag);
  const versatzStunden = Number.parseInt(teile, 10) - 12;
  return new Date(Date.UTC(y, m - 1, d) - versatzStunden * 3_600_000).toISOString() as Instant;
}

/** `03.11.` — a day of a ribbon, without its year. */
export function formatTagKurz(day: BusinessDate): string {
  const [, m, d] = day.split("-");
  return sprache() === "en" ? `${d}/${m}` : `${d}.${m}.`;
}

/** An instant as the seconds since the epoch a chart's time axis plots — for drawing only. */
export function alsAchsenzeit(at: Instant): number {
  return Date.parse(at) / 1000;
}

/** A chart's axis seconds as a wall-clock in Berlin, the zone every Desk instant shows in. */
export function berlinAchsenzeit(sekunden: number, tz: (d: Date, zone: string) => Date): Date {
  return tz(new Date(sekunden * 1000), BERLIN);
}

/** Whole days from `von` to `bis` (now where open) — the width of a band, for drawing only. */
export function tageZwischen(von: Instant, bis: Instant | null): number {
  const ende = bis ? Date.parse(bis) : Date.now();
  return Math.max(0, Math.round((ende - Date.parse(von)) / 86_400_000));
}
