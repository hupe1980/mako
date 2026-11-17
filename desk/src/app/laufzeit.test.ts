import { describe, expect, it } from "vitest";
import { handbuchLink } from "./handbuch";
import { laufzeitAus, STANDARD } from "./laufzeit";

describe("the runtime configuration", () => {
  it("takes the manual's address from the file, without a trailing slash", () => {
    expect(laufzeitAus({ handbuch: "https://docs.example.org/mako/" }).handbuch).toBe(
      "https://docs.example.org/mako",
    );
    expect(laufzeitAus({ handbuch: "/handbuch" }).handbuch).toBe("/handbuch");
  });

  it("keeps the public manual for a missing, empty or non-https address", () => {
    for (const json of [null, {}, { handbuch: "" }, { handbuch: "http://x" }, { handbuch: 3 }])
      expect(laufzeitAus(json)).toEqual(STANDARD);
    expect(laufzeitAus({ handbuch: "javascript:alert(1)" })).toEqual(STANDARD);
  });

  it("offers the demo personas only where the file names them", () => {
    expect(laufzeitAus({}).demo).toBeNull();
    expect(laufzeitAus({ demo: { personas: [] } }).demo).toBeNull();
    const demo = laufzeitAus({
      demo: {
        personas: [
          { id: "nb", name: "Sachbearbeitung Netz" },
          { id: "Bad Id", name: "x" },
          { id: "admin", name: "Administration" },
        ],
        standard: "admin",
      },
    }).demo;
    expect(demo?.personas.map((p) => p.id)).toEqual(["nb", "admin"]);
    expect(demo?.standard).toBe("admin");
    expect(laufzeitAus({ demo: { personas: [{ id: "lf", name: "LF" }] } }).demo?.standard).toBe(
      "lf",
    );
  });

  it("resolves a docs-site reference under that address, and nothing else", () => {
    expect(handbuchLink("@/docs/services/makod.md#policy-x", "https://d.example")).toBe(
      "https://d.example/docs/services/makod/#policy-x",
    );
    expect(handbuchLink("https://elsewhere.example/x", "https://d.example")).toBeNull();
  });
});
