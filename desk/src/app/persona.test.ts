import { describe, expect, it } from "vitest";
import { cookiePersona } from "./persona";

describe("the persona cookie", () => {
  it("reads the persona the edge answers as, and nothing else", () => {
    expect(cookiePersona("mako_persona=lf")).toBe("lf");
    expect(cookiePersona("a=1; mako_persona=nb; b=2")).toBe("nb");
    expect(cookiePersona("mako_persona_x=lf")).toBeNull();
    expect(cookiePersona("")).toBeNull();
  });
});
