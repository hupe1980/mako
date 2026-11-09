// The Desk's runtime configuration: `/config.json` beside the bundle, read
// once from the same origin as every service door. The image ships the
// public defaults; a deployment mounts its own file over
// /usr/share/nginx/html/config.json, so one build serves every deployment.
// A missing or unreadable file, or a key it leaves out, is the default.

import { useQuery } from "@tanstack/react-query";

export interface Laufzeit {
  /** The operator manual a finding's `referenz` opens, without a trailing slash. */
  handbuch: string;
}

export const STANDARD: Laufzeit = { handbuch: "https://hupe1980.github.io/mako" };

/** The configuration as read: an `https:` (or same-origin) manual URL, else the default. */
export function laufzeitAus(json: unknown): Laufzeit {
  const h = (json as { handbuch?: unknown } | null)?.handbuch;
  const gueltig = typeof h === "string" && (h.startsWith("https://") || h.startsWith("/"));
  return { handbuch: gueltig ? h.replace(/\/+$/, "") : STANDARD.handbuch };
}

async function lesen(): Promise<Laufzeit> {
  const res = await fetch("/config.json", { headers: { accept: "application/json" } });
  if (!res.ok) return STANDARD;
  return laufzeitAus(await res.json());
}

/** The runtime configuration; the defaults until (or unless) the file answers. */
export function useLaufzeit(): Laufzeit {
  const q = useQuery({
    queryKey: ["laufzeit"],
    queryFn: () => lesen().catch(() => STANDARD),
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  return q.data ?? STANDARD;
}
