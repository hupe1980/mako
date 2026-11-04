// Business dates are German calendar dates and Fristen run in gesetzlicher
// deutscher Zeit, so every instant renders in Europe/Berlin whatever zone the
// browser runs in, and a business date never passes through a JS `Date` — a
// `Date` built from "2026-01-01" is midnight UTC, which is 31.12.2025 in New York.

import type { BusinessDate, Instant } from "./brands";

const BERLIN = "Europe/Berlin";

const berlin = new Intl.DateTimeFormat("de-DE", {
  timeZone: BERLIN,
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

const berlinTime = new Intl.DateTimeFormat("de-DE", {
  timeZone: BERLIN,
  hour: "2-digit",
  minute: "2-digit",
});

const utc = new Intl.DateTimeFormat("de-DE", {
  timeZone: "UTC",
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

/** `03.11.2026, 14:05` — the instant in Berlin. */
export function formatInstant(at: Instant): string {
  return berlin.format(new Date(at));
}

/** `14:05` — the instant's Berlin wall-clock time. */
export function formatBerlinTime(at: Instant): string {
  return berlinTime.format(new Date(at));
}

/** `03.11.2026, 13:05:00 UTC` — the same instant as the wire carries it. */
export function formatInstantUtc(at: Instant): string {
  return `${utc.format(new Date(at))} UTC`;
}

/** `01.01.2026` from `2026-01-01`, by rearranging the digits. */
export function formatBusinessDate(day: BusinessDate): string {
  const [y, m, d] = day.split("-");
  return `${d}.${m}.${y}`;
}

/** Milliseconds since the epoch for an instant — for ordering and countdowns only. */
export function epochMs(at: Instant): number {
  return Date.parse(at);
}
