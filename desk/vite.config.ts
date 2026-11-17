import { existsSync, readFileSync } from "node:fs";
import type { IncomingMessage } from "node:http";
import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { parse } from "smol-toml";
import type { Plugin, ProxyOptions } from "vite";
import { defineConfig } from "vitest/config";
import { fixturen } from "./scripts/antworten.ts";
import { aufzeichnungen } from "./scripts/aufzeichnung.ts";

const repo = resolve(import.meta.dirname, "..");

// In dev the Vite server is the Desk's gateway: `/svc/<name>` reaches the port
// `deploy/ports.toml` assigns <name>, so there is one port table and no second
// copy. makod is reached with the `dev=` key of its demo configuration
// (`[http] auth_keys`); every other service with the token of the persona the
// Desk's `mako_persona` cookie names (deploy/demo/personas.json), so Cedar
// answers each persona as it would in production. The browser never holds a
// key or a token.
function devProxy(): Record<string, ProxyOptions> {
  const table = parse(readFileSync(resolve(repo, "deploy/ports.toml"), "utf8"));
  const services = table.services as Record<string, number>;
  const tokens = personaTokens();
  return Object.fromEntries(
    Object.entries(services).map(([name, port]) => {
      const prefix = `/svc/${name}`;
      const key = devKey(name);
      return [
        prefix,
        {
          target: `http://127.0.0.1:${port}`,
          rewrite: (path: string) => path.slice(prefix.length),
          ...(key
            ? { headers: { authorization: `Bearer ${key}` } }
            : {
                configure: (proxy) => {
                  proxy.on("proxyReq", (req, incoming: IncomingMessage) => {
                    const token = tokens.forCookie(incoming.headers.cookie ?? "");
                    if (token) req.setHeader("authorization", `Bearer ${token}`);
                  });
                },
              }),
        } satisfies ProxyOptions,
      ];
    }),
  );
}

function devKey(service: string): string | undefined {
  const file = resolve(repo, `deploy/demo/${service}.toml`);
  if (!existsSync(file)) return undefined;
  const http = parse(readFileSync(file, "utf8")).http as { auth_keys?: string[] } | undefined;
  return http?.auth_keys?.find((k) => k.startsWith("dev="))?.slice("dev=".length);
}

interface Personas {
  personas: { id: string; token: string }[];
}

/** The demo sign-in's tokens, by the persona the cookie names; admin when none. */
function personaTokens(): { forCookie: (cookie: string) => string | undefined } {
  const file = resolve(repo, "deploy/demo/personas.json");
  const all = existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as Personas) : null;
  const byId = new Map((all?.personas ?? []).map((p) => [p.id, p.token]));
  return {
    forCookie: (cookie) => {
      const named = /(?:^|;\s*)mako_persona=([a-z0-9-]+)/.exec(cookie)?.[1];
      return byId.get(named ?? "admin") ?? byId.get("admin");
    },
  };
}

// `MAKO_DESK_DEMO=1` (what `just dev` sets): the Desk's runtime configuration
// is the demo's, so the persona switcher is offered.
function demoConfig(): Plugin {
  return {
    name: "mako-demo-config",
    configureServer(server) {
      if (process.env.MAKO_DESK_DEMO !== "1") return;
      const file = resolve(repo, "deploy/demo/desk/config.json");
      server.middlewares.use("/config.json", (_req, res) => {
        res.setHeader("content-type", "application/json");
        res.end(readFileSync(file, "utf8"));
      });
    },
  };
}

// `npm run dev:fixtures` (`just dev-ui`): no service runs — every `/svc/` door
// is answered from the recording the browser checks use (scripts/antworten.ts),
// agentd's runs from desk/fixtures/.
function fixtures(): Plugin {
  return {
    name: "mako-fixtures",
    configureServer(server) {
      server.middlewares.use(aufzeichnungen());
      server.middlewares.use(fixturen());
    },
  };
}

// The proxy is the dev server's alone: a production build reads no port table
// and no service config — the gateway (`deploy/gateway.yaml`) routes `/svc/`.
export default defineConfig(({ command, mode }) => ({
  plugins: [react(), tailwindcss(), demoConfig(), ...(mode === "fixtures" ? [fixtures()] : [])],
  ...(command === "serve"
    ? {
        server: {
          port: 5173,
          strictPort: true,
          ...(mode === "fixtures" ? {} : { proxy: devProxy() }),
        },
      }
    : {}),
  build: { target: "es2023", sourcemap: true },
  test: {
    environment: "jsdom",
    setupFiles: ["src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
