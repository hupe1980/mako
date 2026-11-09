import { describe, expect, it } from "vitest";
import { antwortTeile } from "./antwort";
import {
  formatAnteil,
  hilfreichText,
  MINDEST_URTEILE,
  vorschlagAufschluesselung,
  vorschlagText,
} from "./bilanz";
import type { BusinessDate, Decimal, Instant } from "./brands";
import { formatBusinessDate, formatInstant, formatInstantUtc } from "./date";
import { faktKoerper } from "./fakt";
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

describe("a skill's record: a share only from five verdicts", () => {
  it("sets the threshold at five", () => {
    expect(MINDEST_URTEILE).toBe(5);
  });

  it("says a share of „hilfreich“ only from five verdicts, the count below, words for none", () => {
    expect(hilfreichText({ ja: 0, nein: 0 })).toBe("Noch keine Bewertungen");
    expect(hilfreichText({ ja: 1, nein: 0 })).toBe("1 Bewertung");
    expect(hilfreichText({ ja: 1, nein: 1 })).toBe("2 Bewertungen");
    expect(hilfreichText({ ja: 4, nein: 0 })).toBe("4 Bewertungen");
    expect(hilfreichText({ ja: 5, nein: 0 })).toMatch(/^100\s%\shilfreich · 5 Bewertungen$/);
    expect(hilfreichText({ ja: 6, nein: 1 })).toMatch(/^86\s%\shilfreich · 7 Bewertungen$/);
    // Five verdicts all „nein“ is a real share, and shown as one.
    expect(hilfreichText({ ja: 0, nein: 5 })).toMatch(/^0\s%\shilfreich · 5 Bewertungen$/);
  });

  it("states the proposals taken as „n von m“ only from five decided", () => {
    expect(vorschlagText({})).toBe("Noch kein Vorschlag entschieden");
    expect(vorschlagText({ dismissed: 1 })).toBe("1 Vorschlag entschieden");
    expect(vorschlagText({ sent: 2, edited: 2 })).toBe("4 Vorschläge entschieden");
    expect(vorschlagText({ sent: 14, edited: 2, dismissed: 1 })).toBe(
      "14 von 17 Vorschlägen übernommen",
    );
    expect(vorschlagText({ edited: 5 })).toBe("0 von 5 Vorschlägen übernommen");
    expect(vorschlagAufschluesselung({ sent: 1234, dismissed: 1 })).toBe(
      "1.234 übernommen · 0 angepasst · 1 verworfen",
    );
  });

  it("rounds a share to a whole percent in German", () => {
    expect(formatAnteil(1, 3)).toMatch(/^33\s%$/);
    expect(formatAnteil(2, 3)).toMatch(/^67\s%$/);
  });
});

describe("antwortTeile", () => {
  it("states the code with the catalogue's wording", () => {
    expect(
      antwortTeile({
        antwortcode: "A02",
        codeliste: "E_0622",
        bedeutung: "Marktlokation nimmt nicht teil",
      }),
    ).toEqual({ code: "A02", text: "Marktlokation nimmt nicht teil" });
  });
  it("names the list, not a meaning, when the catalogue has none", () => {
    expect(antwortTeile({ antwortcode: "Z99", codeliste: "E_0622", bedeutung: null })).toEqual({
      code: "Z99",
      text: "aus E_0622, ohne Wortlaut im Katalog",
    });
  });
  it("is nothing on a message that gives no answer", () => {
    expect(antwortTeile({ antwortcode: null })).toBeNull();
  });
});

describe("faktKoerper", () => {
  it("sends the Abrechnungsintervall as a number and the EnFG Grundlage as listed", () => {
    const fakten = [
      { feld: "abrechnungsintervall_monate", bezeichnung: "", werte: [] },
      { feld: "enfg", bezeichnung: "", werte: [] },
      { feld: "bilanzkreis", bezeichnung: "", werte: [] },
    ];
    expect(
      faktKoerper(fakten, { abrechnungsintervall_monate: "12", enfg: "ZF9+ZU6", bilanzkreis: " " }),
    ).toEqual({ abrechnungsintervall_monate: 12, enfg: "ZF9+ZU6" });
  });
});
