import { describe, expect, it } from "vitest";
import type { BusinessDate, Decimal, Instant } from "./brands";
import { formatBusinessDate, formatInstant, formatInstantUtc } from "./date";
import { band, distance, seit } from "./frist";
import { formatCent, formatDecimal, formatEuro } from "./money";

const instant = (s: string) => s as Instant;

describe("dates (tests run with TZ=America/New_York, see test-setup.ts)", () => {
  it("runs in a zone that is not Berlin", () => {
    expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe("America/New_York");
  });

  it("renders a business date without moving it a day", () => {
    expect(formatBusinessDate("2026-01-01" as BusinessDate)).toBe("01.01.2026");
  });

  it("renders a Lieferbeginn instant as Berlin midnight in winter", () => {
    // 23:00 UTC on 31.12. is 00:00 MEZ on 01.01.
    expect(formatInstant(instant("2025-12-31T23:00:00Z"))).toBe("01.01.2026, 00:00");
  });

  it("renders summer time as MESZ", () => {
    expect(formatInstant(instant("2026-06-30T22:00:00Z"))).toBe("01.07.2026, 00:00");
  });

  it("keeps the UTC reading available", () => {
    expect(formatInstantUtc(instant("2025-12-31T23:00:00Z"))).toBe("31.12.2025, 23:00:00 UTC");
  });
});

describe("amounts", () => {
  it("formats every digit the service sent, without float error", () => {
    expect(formatDecimal("0.10000" as Decimal)).toBe("0,1");
    expect(formatEuro("1234.56789" as Decimal)).toBe("1.234,56789 €");
    expect(formatDecimal("0.30000000000000004441" as Decimal)).toBe("0,30000000000000004441");
  });

  it("formats a ledger's cents as euro, the sign kept", () => {
    expect(formatCent(900)).toBe(formatEuro("9.00" as Decimal));
    expect(formatCent(-123456)).toBe(formatEuro("-1234.56" as Decimal));
    expect(formatCent(5)).toBe(formatEuro("0.05" as Decimal));
  });
});

describe("Frist-Uhr", () => {
  const due = instant("2026-11-03T12:00:00Z");
  const at = (iso: string) => Date.parse(iso);

  it("calls a lapsed deadline lapsed", () => {
    expect(distance(due, at("2026-11-03T14:00:00Z"))).toEqual({
      urgency: "lapsed",
      text: "abgelaufen vor 2 h",
    });
  });

  it("is critical inside two hours", () => {
    expect(distance(due, at("2026-11-03T10:30:00Z"))).toEqual({
      urgency: "critical",
      text: "in 1 h 30 min",
    });
  });

  it("is calm beyond a day", () => {
    expect(distance(due, at("2026-11-01T12:00:00Z")).urgency).toBe("calm");
  });
});

describe("how long ago", () => {
  it("says how long something has waited", () => {
    const now = Date.parse("2026-11-07T12:00:00Z");
    expect(seit(instant("2026-11-04T12:00:00Z"), now)).toBe("seit 3 Tagen");
    expect(seit(instant("2026-11-07T10:00:00Z"), now)).toBe("seit 2 h");
  });
});

describe("the Frist band", () => {
  // Saturday 28.03.2026, the night before the clocks change: Berlin is still
  // UTC+1, so 22:59Z is Saturday and 23:30Z already Sunday; by 04.04. it is
  // UTC+2, so 21:00Z is 23:00 that Saturday, seven Berlin days on.
  const now = Date.parse("2026-03-28T10:00:00Z");
  it("reads lapsed, today, the next seven Berlin days, later and none", () => {
    expect(band("2026-03-28T09:59:00Z" as Instant, now)).toBe("ueberfaellig");
    expect(band("2026-03-28T22:59:00Z" as Instant, now)).toBe("heute");
    expect(band("2026-03-28T23:30:00Z" as Instant, now)).toBe("woche");
    expect(band("2026-04-04T21:00:00Z" as Instant, now)).toBe("woche");
    expect(band("2026-04-05T22:30:00Z" as Instant, now)).toBe("spaeter");
    expect(band(null, now)).toBe("ohne");
  });
});
