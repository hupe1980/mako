import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { tileState } from "../views/Ueberblick";
import { Tile } from "./Tile";

describe("a tile", () => {
  it("is named by what it shows", () => {
    render(
      <Tile
        label="Fristen überschritten"
        source="processd"
        href="/fristen"
        state={{ kind: "count", n: 0 }}
      />,
    );
    const link = screen.getByRole("link");
    // label-content-name-mismatch: the accessible name contains the visible text.
    expect(link.textContent).toContain("Fristen überschritten");
    expect(link.getAttribute("aria-label")).toBeNull();
  });

  it("shows no figure the service did not return", () => {
    const state = tileState(
      { data: {}, isError: false, error: null },
      "makod",
      (d: { gesamt?: number }) => ({
        n: d.gesamt as number,
      }),
    );
    expect(state.kind).toBe("failed");
  });
});
