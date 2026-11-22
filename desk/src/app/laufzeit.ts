// The Desk's runtime configuration: `/config.json` beside the bundle, read
// once from the same origin as every service door. The image ships the
// public defaults; a deployment mounts its own file over
// /usr/share/nginx/html/config.json, so one build serves every deployment.
// A missing or unreadable file, or a key it leaves out, is the default.

import { useQuery } from "@tanstack/react-query";

/** A persona of the demo sign-in: who the Desk works as, by the edge's token. */
export interface Persona {
  id: string;
  name: string;
  /** The Marktrollen the persona's token carries (`NB`, `LF`, …). */
  rollen?: string[];
}

/** The demo sign-in (deploy/demo/desk/config.json); absent in every real deployment. */
export interface Demo {
  personas: Persona[];
  /** The persona a Desk without a choice works as. */
  standard: string;
}

export interface Laufzeit {
  /** The operator manual a finding's `referenz` opens, without a trailing slash. */
  handbuch: string;
  demo: Demo | null;
}

export const STANDARD: Laufzeit = { handbuch: "https://hupe1980.github.io/mako", demo: null };

const PERSONA_ID = /^[a-z0-9-]{1,32}$/;

/** The demo personas as read: each with an id and a name, else no demo at all. */
function demoAus(json: unknown): Demo | null {
  const d = json as { personas?: unknown; standard?: unknown } | null | undefined;
  if (!d || !Array.isArray(d.personas)) return null;
  const personas = d.personas.flatMap((p: unknown) => {
    const { id, name } = (p ?? {}) as { id?: unknown; name?: unknown };
    return typeof id === "string" && PERSONA_ID.test(id) && typeof name === "string" && name
      ? [{ id, name }]
      : [];
  });
  const first = personas[0];
  if (!first) return null;
  const standard = personas.some((p) => p.id === d.standard) ? (d.standard as string) : first.id;
  return { personas, standard };
}

/** The configuration as read: an `https:` (or same-origin) manual URL, else the default. */
export function laufzeitAus(json: unknown): Laufzeit {
  const j = json as { handbuch?: unknown; demo?: unknown } | null;
  const h = j?.handbuch;
  const gueltig = typeof h === "string" && (h.startsWith("https://") || h.startsWith("/"));
  return {
    handbuch: gueltig ? h.replace(/\/+$/, "") : STANDARD.handbuch,
    demo: demoAus(j?.demo),
  };
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
