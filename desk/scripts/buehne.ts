// The stage the Desk's browser checks run on (`budgets.ts`, `zugang.ts`): the
// production build served by `vite preview`, agentd's stream played from
// desk/fixtures/ by the recorded-run middleware (`aufzeichnung.ts`), and every
// other service answer recorded in the shape its mcp.snapshot.json declares.
// No service has to run, and nothing here measures — it only sets the scene.

import { resolve } from "node:path";
import type { BrowserContext, Page } from "playwright";
import { build, preview } from "vite";
import { beantworten } from "./antworten.ts";
import { aufzeichnungen } from "./aufzeichnung.ts";

const desk = resolve(import.meta.dirname, "..");
const FIXTURES = resolve(desk, "fixtures");

/**
 * Record every service answer in `context`. A question starts the recorded
 * run `run` — its stream is left to the preview server, which plays the
 * fixture with its delays, so the browser waits on a real event stream.
 */
export async function aufnehmen(context: BrowserContext, run: string): Promise<void> {
  await context.route(
    (url) => url.pathname.startsWith("/svc/") && !url.pathname.endsWith("/schritte"),
    async (route) => {
      const req = route.request();
      const a = beantworten(
        { pathname: new URL(req.url()).pathname, method: req.method(), body: req.postData() },
        run,
      );
      return route.fulfill({
        status: a.status,
        ...(a.contentType ? { contentType: a.contentType } : {}),
        ...(a.headers ? { headers: a.headers } : {}),
        ...(a.body !== undefined ? { body: a.body } : {}),
      });
    },
  );
}

/** A new tab on the stage, in the Rolle Netz, its zone and language German. */
export async function seite(context: BrowserContext): Promise<Page> {
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("mako-desk.rolle", "netz"));
  return page;
}

export const KONTEXT = {
  viewport: { width: 1280, height: 800 },
  locale: "de-DE",
  timezoneId: "Europe/Berlin",
} as const;

/** Build the Desk and serve the build, with the recorded runs in front of it. */
export async function buehne(): Promise<{ base: string; schliessen: () => Promise<void> }> {
  // A production build whatever ran before it in this process: a dev server
  // leaves NODE_ENV at development, and Vite then builds with DEV true.
  process.env.NODE_ENV = "production";
  await build({ root: desk, mode: "production", logLevel: "error" });
  // The build is the config file's; the preview takes none of it, so no dev
  // proxy stands behind the stage and no request can reach a running service.
  const server = await preview({
    root: desk,
    configFile: false,
    logLevel: "error",
    preview: { port: 0, host: "127.0.0.1" },
    plugins: [
      {
        name: "aufzeichnungen",
        configurePreviewServer(s) {
          s.middlewares.use(aufzeichnungen(FIXTURES));
        },
      },
    ],
  });
  const base = server.resolvedUrls?.local[0];
  if (!base) throw new Error("vite preview hat keine Adresse");
  return {
    base,
    schliessen: () =>
      new Promise<void>((ok, fehler) => server.httpServer.close((e) => (e ? fehler(e) : ok()))),
  };
}
