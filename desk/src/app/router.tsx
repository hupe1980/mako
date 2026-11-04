import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  type RouterHistory,
} from "@tanstack/react-router";
import { lazy } from "react";
import { AssistentView } from "../views/Assistent";
import { FreigabenView } from "../views/Freigaben";
import { FristenView } from "../views/Fristen";
import { HeuteView } from "../views/Heute";
import { KundeView } from "../views/Kunde";
import { KundenView } from "../views/Kunden";
import { ProzessView } from "../views/Prozess";
import { UeberblickView } from "../views/Ueberblick";
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
  validateSearch: (search: Record<string, unknown>): { key?: string } =>
    typeof search.key === "string" && search.key.trim() ? { key: search.key.trim() } : {},
  component: ProzessView,
});

const heute = createRoute({
  getParentRoute: () => root,
  path: "/heute",
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
  component: FristenView,
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
    ? [index, ueberblick, freigaben, fristen, heute, prozess, kunden, kunde, assistent, gallery]
    : [index, ueberblick, freigaben, fristen, heute, prozess, kunden, kunde, assistent],
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
