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

  it("resolves a docs-site reference under that address, and nothing else", () => {
    expect(handbuchLink("@/docs/services/makod.md#policy-x", "https://d.example")).toBe(
      "https://d.example/docs/services/makod/#policy-x",
    );
    expect(handbuchLink("https://elsewhere.example/x", "https://d.example")).toBeNull();
  });
});
