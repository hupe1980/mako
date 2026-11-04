import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { type Rolle, RolleProvider, useRolle } from "./rollen";

function Probe() {
  const { rolle, wear } = useRolle();
  return (
    <button type="button" onClick={() => wear("lieferant")}>
      {rolle}
    </button>
  );
}

function mount(held: Rolle[], queries = new QueryClient()) {
  render(
    <QueryClientProvider client={queries}>
      <RolleProvider held={held}>
        <Probe />
      </RolleProvider>
    </QueryClientProvider>,
  );
  return queries;
}

afterEach(() => {
  document.body.innerHTML = "";
  localStorage.clear();
});

describe("Rollen", () => {
  it("clears every cached read of the Rolle it leaves", () => {
    const queries = mount(["netz", "lieferant"]);
    queries.setQueryData(["netz", "makod", "get_process", "51238696012"], { netz: "data" });
    expect(document.documentElement.dataset.rolle).toBe("netz");

    act(() => screen.getByRole("button").click());

    expect(screen.getByRole("button").textContent).toBe("lieferant");
    expect(document.documentElement.dataset.rolle).toBe("lieferant");
    expect(queries.getQueryCache().getAll()).toHaveLength(0);
  });

  it("cannot wear a Rolle the person does not hold", () => {
    const queries = mount(["netz"]);
    queries.setQueryData(["netz", "x"], 1);

    act(() => screen.getByRole("button").click());

    expect(screen.getByRole("button").textContent).toBe("netz");
    expect(queries.getQueryCache().getAll()).toHaveLength(1);
  });

  it("offers the switcher shortcut ⌃2 for the second Rolle", () => {
    mount(["netz", "lieferant"]);
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "2", ctrlKey: true }));
    });
    expect(screen.getByRole("button").textContent).toBe("lieferant");
  });
});
