import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { parse } from "smol-toml";
import type { ProxyOptions } from "vite";
import { defineConfig } from "vitest/config";

const repo = resolve(import.meta.dirname, "..");

// In dev the Vite server is the Desk's gateway: `/svc/<name>` reaches the port
// `dev/ports.toml` assigns <name>, so there is one port table and no second copy.
// A service whose dev config requires a named key (makod's `[http] auth_keys`)
// gets its `dev=` key attached here — the browser never holds a key.
function devProxy(): Record<string, ProxyOptions> {
  const table = parse(readFileSync(resolve(repo, "dev/ports.toml"), "utf8"));
  const services = table.services as Record<string, number>;
  return Object.fromEntries(
    Object.entries(services).map(([name, port]) => {
      const prefix = `/svc/${name}`;
      const key = devKey(name);
      return [
        prefix,
        {
          target: `http://127.0.0.1:${port}`,
          rewrite: (path: string) => path.slice(prefix.length),
          ...(key ? { headers: { authorization: `Bearer ${key}` } } : {}),
        },
      ];
    }),
  );
}

function devKey(service: string): string | undefined {
  const file = resolve(repo, `dev/config/${service}.toml`);
  if (!existsSync(file)) return undefined;
  const http = parse(readFileSync(file, "utf8")).http as { auth_keys?: string[] } | undefined;
  return http?.auth_keys?.find((k) => k.startsWith("dev="))?.slice("dev=".length);
}

// The proxy is the dev server's alone: a production build reads no port table
// and no service config — the gateway (`deploy/gateway.yaml`) routes `/svc/`.
export default defineConfig(({ command }) => ({
  plugins: [react(), tailwindcss()],
  ...(command === "serve" ? { server: { port: 5173, strictPort: true, proxy: devProxy() } } : {}),
  build: { target: "es2023", sourcemap: true },
  test: {
    environment: "jsdom",
    setupFiles: ["src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
