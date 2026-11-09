import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import abgelehntJson from "../../fixtures/probe-44004-abgelehnt.json?raw";
import { RolleProvider } from "../app/rollen";
import { Probelauf } from "./Einrichtung";

// The rehearsal panel renders makod's answer as it came: the verdict is
// `urteil`, the reason `grund`, the segments and their names and findings the
// service's. The fixture is a real makod answer — a 44004 Abmeldung whose
// Transaktionsgrund `E03` its column does not admit.

const ABGELEHNT = JSON.parse(abgelehntJson) as Record<string, unknown>;

/** makod answers the probe with `status` and `body`; every request is recorded. */
function makod(status: number, body: unknown) {
  const requests: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit) => {
      requests.push({ url, body: JSON.parse(String(init.body)) });
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      });
    }),
  );
  return requests;
}

function show() {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RolleProvider>
        <Probelauf />
      </RolleProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the Probelauf", () => {
  it("posts the chosen command's facts to makod's probe and shows its verdict and segments", async () => {
    const requests = makod(200, ABGELEHNT);
    show();
    fireEvent.click(screen.getByRole("radio", { name: /Abmeldung Gas · 44004/ }));
    fireEvent.click(screen.getByRole("button", { name: /Probelauf starten/ }));

    await screen.findByText(/^Abgelehnt/);
    expect(requests).toHaveLength(1);
    expect(requests[0]?.url).toBe("/svc/makod/api/v1/probe/geli.lieferende.anmelden");
    expect(requests[0]?.body).toEqual({
      payload: { malo_id: "51238696012", process_date: "20261231", transaktionsgrund: "ZT5" },
    });

    // The reason is makod's sentence, word for word.
    expect(screen.getByText(ABGELEHNT.grund as string)).toBeTruthy();
    expect(screen.getByText("Nichts wurde gesendet oder gespeichert.")).toBeTruthy();
    // Annotated against the profile of the message's own format version.
    expect(screen.getByText("annotiert gegen FV2026-10-01 · AHB 1.2")).toBeTruthy();

    const grid = screen.getByRole("grid", { name: "Segmente der Nachricht mit ihren AHB-Namen" });
    const rows = within(grid).getAllByRole("row");
    // A header row and one row per segment, UNH to UNT.
    expect(rows).toHaveLength(12);
    const sts = within(grid).getByRole("rowheader", { name: /Transaktionsgrund/ });
    const row = sts.closest("[role=row]") as HTMLElement;
    // The failing data element is marked where it sits, and named for a screen reader.
    const mark = row.querySelector("mark");
    expect(mark?.textContent).toContain("E03");
    expect(mark?.textContent).toContain("fehlerhaft: Datenelement 3");
    expect(within(row).getByText(/admits E01, Z33/)).toBeTruthy();
  });

  it("moves focus to the segment a finding names", async () => {
    makod(200, ABGELEHNT);
    show();
    fireEvent.click(screen.getByRole("button", { name: /Probelauf starten/ }));
    const jump = await screen.findByRole("button", { name: /Zu STS, Position 8/ });
    fireEvent.click(jump);
    await waitFor(() =>
      expect((document.activeElement as HTMLElement | null)?.getAttribute("data-key")).toBe("8"),
    );
  });

  it("shows a passing verdict from `urteil` alone — the Desk decides nothing", async () => {
    makod(200, {
      ...ABGELEHNT,
      urteil: "besteht",
      stufe: null,
      grund: null,
      nachrichten: [],
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: /Probelauf starten/ }));
    expect(await screen.findByText(/^Besteht/)).toBeTruthy();
    expect(screen.getByText(/passiert die Prüfung vor dem Versand/)).toBeTruthy();
  });

  it("shows makod's refusal to rehearse in its own words", async () => {
    makod(422, {
      error: "Unprocessable Entity",
      code: "role_not_configured",
      detail: "This deployment does not declare the Marktrolle LF, which the command needs.",
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: /Probelauf starten/ }));
    expect(await screen.findByText("Probelauf nicht möglich")).toBeTruthy();
    expect(screen.getByText(/does not declare the Marktrolle LF/)).toBeTruthy();
  });

  it("sends nothing while the facts are not a JSON object", () => {
    const requests = makod(200, ABGELEHNT);
    show();
    fireEvent.change(screen.getByRole("textbox", { name: "Angaben (JSON)" }), {
      target: { value: "{ nicht json" },
    });
    expect(screen.getByText("Die Angaben sind kein JSON-Objekt.")).toBeTruthy();
    const start = screen.getByRole("button", { name: /Probelauf starten/ });
    expect(start.hasAttribute("disabled") || start.getAttribute("data-disabled") !== null).toBe(
      true,
    );
    fireEvent.click(start);
    expect(requests).toHaveLength(0);
  });
});
