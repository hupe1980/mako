import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import fixture from "../../fixtures/shadow-review.json";
import { RolleProvider } from "../app/rollen";
import { createDeskRouter } from "../app/router";

async function open(answer: () => Response, write?: (body: unknown) => Response) {
  localStorage.setItem("mako-desk.rolle", "netz");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url.endsWith("/resolve") && write) return write(JSON.parse(String(init?.body)));
      if (url.endsWith("/api/v1/shadow/review")) return answer();
      if (url === "/config.json") return Response.json({});
      return Response.json({ result: { content: [], structuredContent: {} } });
    }),
  );
  const router = createDeskRouter(createMemoryHistory({ initialEntries: ["/schattenbetrieb"] }));
  await router.load();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RolleProvider>
        <RouterProvider router={router} />
      </RolleProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.clear();
});

it.each([
  [403, "Für diese Kohorte fehlt dir die Leseberechtigung."],
  [503, "Die Prüfinstallation ist nicht erreichbar oder nicht eingerichtet."],
])("reports an unavailable read (%i) without displaying a zero cohort", async (status, text) => {
  await open(() => Response.json({ code: "shadow_forbidden" }, { status: Number(status) }));
  expect(await screen.findByText(text)).toBeTruthy();
  expect(screen.queryByText(/0 von 0 Eingängen/)).toBeNull();
});

it("distinguishes a verified empty interval", async () => {
  await open(() =>
    Response.json({
      ...fixture,
      cohort: { ...fixture.cohort, rows: [] },
      eligible: 0,
      assessed: 0,
      unassessed: 0,
      different: 0,
      open_differences: 0,
      timing_unassessed: 0,
    }),
  );
  expect(await screen.findByText("Im geprüften Zeitraum gibt es keine Eingänge.")).toBeTruthy();
  expect(screen.getByText(/0 von 0 Eingängen/)).toBeTruthy();
});

it("a named resolver submits the inspected generation and can recover from a stale decision", async () => {
  let body: unknown;
  await open(
    () => Response.json({ ...fixture, can_resolve: true }),
    (b) => {
      body = b;
      return Response.json({ code: "shadow_revision_changed" }, { status: 412 });
    },
  );
  fireEvent.change(await screen.findByRole("textbox", { name: "Begründung" }), {
    target: { value: "Beleg geprüft" },
  });
  fireEvent.change(screen.getByRole("textbox", { name: "Belegreferenz" }), {
    target: { value: "Archiv 42" },
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Bestandsantwort als fehlerhaft bestätigen" }),
  );
  expect(await screen.findByText(/Die Belege haben sich geändert/)).toBeTruthy();
  expect(body).toEqual({
    generation: fixture.generation,
    revision: fixture.revision,
    reason: "Beleg geprüft",
    evidence: "Archiv 42",
  });
});

it("a reader sees the difference and has no resolution command", async () => {
  await open(() => Response.json(fixture));
  expect(await screen.findByText("A07")).toBeTruthy();
  expect(
    screen.queryByRole("button", { name: "Bestandsantwort als fehlerhaft bestätigen" }),
  ).toBeNull();
});
