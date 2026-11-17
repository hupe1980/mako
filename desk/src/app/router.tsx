import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  type RouterHistory,
} from "@tanstack/react-router";
import { lazy } from "react";

const LokationView = lazy(() =>
  import("../views/Lokation").then((m) => ({ default: m.LokationView })),
);

const GegenparteienView = lazy(() =>
  import("../views/Gegenpartei").then((m) => ({ default: m.GegenparteienView })),
);

const GegenparteiView = lazy(() =>
  import("../views/Gegenpartei").then((m) => ({ default: m.GegenparteiView })),
);

const NachrechnenView = lazy(() =>
  import("../views/Nachrechnen").then((m) => ({ default: m.NachrechnenView })),
);

const FreigabenView = lazy(() =>
  import("../views/FreigabenListe").then((m) => ({ default: m.FreigabenView })),
);

const UeberblickView = lazy(() =>
  import("../views/Ueberblick").then((m) => ({ default: m.UeberblickView })),
);
const ProzessView = lazy(() =>
  import("../views/Prozess").then((m) => ({ default: m.ProzessView })),
);
const KundenView = lazy(() => import("../views/Kunden").then((m) => ({ default: m.KundenView })));
const KundeView = lazy(() => import("../views/Kunde").then((m) => ({ default: m.KundeView })));
const FristenView = lazy(() =>
  import("../views/Fristen").then((m) => ({ default: m.FristenView })),
);
const EinrichtungView = lazy(() =>
  import("../views/Einrichtung").then((m) => ({ default: m.EinrichtungView })),
);
const DeadLettersView = lazy(() =>
  import("../views/DeadLetters").then((m) => ({ default: m.DeadLettersView })),
);
const AssistentView = lazy(() =>
  import("../views/Assistent").then((m) => ({ default: m.AssistentView })),
);

import { QUELLE_IDS, type QuelleId } from "../inbox/model";
import { HeuteView } from "../views/Heute";
import { ROLLEN, useRolle } from "./rollen";
import { Shell } from "./Shell";

const root = createRootRoute({ component: Shell });

function Home() {
  const { rolle } = useRolle();
  return <Navigate to={ROLLEN[rolle].home} replace />;
}

const index = createRoute({
  getParentRoute: () => root,
  path: "/",
  component: Home,
});

const prozess = createRoute({
  getParentRoute: () => root,
  path: "/prozess",
  validateSearch: (search: Record<string, unknown>): { key?: string; p?: string } => ({
    ...(typeof search.key === "string" && search.key.trim() ? { key: search.key.trim() } : {}),
    ...(typeof search.p === "string" && search.p ? { p: search.p } : {}),
  }),
  component: ProzessView,
});

const heute = createRoute({
  getParentRoute: () => root,
  path: "/heute",
  validateSearch: (search: Record<string, unknown>): { quelle?: QuelleId } =>
    typeof search.quelle === "string" && (QUELLE_IDS as readonly string[]).includes(search.quelle)
      ? { quelle: search.quelle as QuelleId }
      : {},
  component: HeuteView,
});

const kunden = createRoute({
  getParentRoute: () => root,
  path: "/kunden",
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q.trim() ? { q: search.q } : {},
  component: KundenView,
});

const kunde = createRoute({
  getParentRoute: () => root,
  path: "/kunde/$id",
  component: KundeView,
});

const ueberblick = createRoute({
  getParentRoute: () => root,
  path: "/ueberblick",
  component: UeberblickView,
});

const assistent = createRoute({
  getParentRoute: () => root,
  path: "/assistent",
  component: AssistentView,
});

const freigaben = createRoute({
  getParentRoute: () => root,
  path: "/freigaben",
  validateSearch: (search: Record<string, unknown>): { id?: string } =>
    typeof search.id === "string" && search.id ? { id: search.id } : {},
  component: FreigabenView,
});

const fristen = createRoute({
  getParentRoute: () => root,
  path: "/fristen",
  validateSearch: (search: Record<string, unknown>): { id?: string } =>
    typeof search.id === "string" && search.id ? { id: search.id } : {},
  component: FristenView,
});

const einrichtung = createRoute({
  getParentRoute: () => root,
  path: "/einrichtung",
  component: EinrichtungView,
});

const lokation = createRoute({
  getParentRoute: () => root,
  path: "/lokation/$id",
  validateSearch: (search: Record<string, unknown>): { knoten?: string } =>
    typeof search.knoten === "string" && search.knoten ? { knoten: search.knoten } : {},
  component: LokationView,
});

const gegenparteien = createRoute({
  getParentRoute: () => root,
  path: "/gegenparteien",
  component: GegenparteienView,
});

const gegenpartei = createRoute({
  getParentRoute: () => root,
  path: "/gegenpartei/$mpId/$rolle",
  component: GegenparteiView,
});

const rechnung = createRoute({
  getParentRoute: () => root,
  path: "/rechnung/$id",
  component: NachrechnenView,
});

const deadLetters = createRoute({
  getParentRoute: () => root,
  path: "/dead-letters",
  component: DeadLettersView,
});

// The gallery is a development page: production builds never import it.
const Gallery = lazy(() => import("../gallery/Gallery").then((m) => ({ default: m.Gallery })));

const gallery = createRoute({
  getParentRoute: () => root,
  path: "/dev/gallery",
  component: Gallery,
});

// Search params stay strings. The router's default parses them as JSON, which
// turns `?key=51238696012` into a number and a Vorgangsnummer `0042…` into 42 —
// an identifier in this domain is never a number.
const parseSearch = (search: string) =>
  Object.fromEntries(new URLSearchParams(search)) as Record<string, string>;
const stringifySearch = (search: Record<string, unknown>) => {
  const params = new URLSearchParams(
    Object.entries(search).flatMap(([k, v]) => (v === undefined ? [] : [[k, String(v)]])),
  ).toString();
  return params ? `?${params}` : "";
};

const routeTree = root.addChildren(
  import.meta.env.DEV
    ? [
        index,
        ueberblick,
        freigaben,
        fristen,
        deadLetters,
        rechnung,
        gegenpartei,
        gegenparteien,
        lokation,
        heute,
        prozess,
        kunden,
        kunde,
        assistent,
        einrichtung,
        gallery,
      ]
    : [
        index,
        ueberblick,
        freigaben,
        fristen,
        deadLetters,
        rechnung,
        gegenpartei,
        gegenparteien,
        lokation,
        heute,
        prozess,
        kunden,
        kunde,
        assistent,
        einrichtung,
      ],
);

/** A Desk router — the app makes one; each test makes its own. */
export function createDeskRouter(history?: RouterHistory) {
  return createRouter({
    parseSearch,
    stringifySearch,
    routeTree,
    defaultPreload: "intent",
    ...(history ? { history } : {}),
  });
}

export const router = createDeskRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
