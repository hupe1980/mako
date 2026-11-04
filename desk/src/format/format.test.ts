import { describe, expect, it } from "vitest";
import type { BusinessDate, Decimal, Instant } from "./brands";
import { formatBusinessDate, formatInstant, formatInstantUtc } from "./date";
import { distance } from "./frist";
import { formatDecimal, formatEuro } from "./money";

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
