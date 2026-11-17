import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The token sheet held to two rules a reviewer cannot see in a diff: one
// accent hue for the whole Desk — the AI colour is the only other hue, and it
// belongs to agent content — and WCAG 2.2 AA (4.5:1) for every text colour on
// every surface it is set on, in both appearances.

const CSS = readFileSync(resolve(import.meta.dirname, "tokens.css"), "utf8");

type Rgba = [number, number, number, number];

/** Every `selector { … }` block with the custom properties it declares. */
function blocks(css: string): { selector: string; decls: Map<string, string> }[] {
  const out: { selector: string; decls: Map<string, string> }[] = [];
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (const m of stripped.matchAll(re)) {
    const decls = new Map<string, string>();
    for (const d of (m[2] ?? "").matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g))
      decls.set(d[1] ?? "", (d[2] ?? "").trim());
    out.push({ selector: (m[1] ?? "").split(";").pop()?.trim() ?? "", decls });
  }
  return out;
}

const ROOT = blocks(CSS).find((b) => b.selector === ":root")?.decls ?? new Map<string, string>();

function colour(text: string): Rgba {
  const hex = /^#([0-9a-f]{6})$/i.exec(text.trim());
  if (hex?.[1]) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgb = /^rgb\((\d+) (\d+) (\d+)(?: \/ ([\d.]+))?\)$/.exec(text.trim());
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), Number(rgb[4] ?? 1)];
  throw new Error(`not a colour the test reads: ${text}`);
}

/** A token's light and dark value. */
function pair(name: string, decls = ROOT): [Rgba, Rgba] {
  const v = decls.get(name);
  const m = v && /^light-dark\((.+),\s*(#[0-9a-f]{6}|rgb\([^)]*\))\)$/i.exec(v);
  if (!m?.[1] || !m[2]) throw new Error(`${name} is not one light-dark() pair: ${v}`);
  return [colour(m[1]), colour(m[2])];
}

function over(top: Rgba, under: Rgba): Rgba {
  const a = top[3];
  return [0, 1, 2].map((i) => (top[i] ?? 0) * a + (under[i] ?? 0) * (1 - a)).concat(1) as Rgba;
}

function luminance([r, g, b]: Rgba): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function contrast(a: Rgba, b: Rgba): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

function hue([r, g, b]: Rgba): number {
  const [R, G, B] = [r / 255, g / 255, b / 255];
  const max = Math.max(R, G, B);
  const d = max - Math.min(R, G, B);
  if (d === 0) return Number.NaN;
  const h = max === R ? ((G - B) / d) % 6 : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
  return (h * 60 + 360) % 360;
}

const SURFACES = ["--bg", "--surface", "--surface-raised", "--sidebar"];
const TEXTS = [
  "--text",
  "--text-2",
  "--accent",
  "--critical",
  "--soon",
  "--done",
  "--info",
  "--ai",
];
/** Status text on its own tint, the tint laid over each surface. */
const TINTED: [string, string][] = [
  ["--critical", "--critical-fill"],
  ["--soon", "--soon-fill"],
  ["--done", "--done-fill"],
  ["--info", "--info-fill"],
  ["--ai", "--ai-fill"],
];

/**
 * The colour tokens and what each is for. A new chromatic token is a new
 * meaning and is added here with one; a second accent cannot slip in as a
 * „link" or a „Rolle" colour.
 */
const CHROMATIC: Record<string, string> = {
  "--accent": "the one accent: the primary button, the selection, a link",
  "--critical": "a Frist past or a failure",
  "--soon": "a Frist near",
  "--done": "done",
  "--info": "in progress",
  "--ai": "agent content only",
};

describe("the design tokens", () => {
  it("declare the accent once, on :root, and no selector re-colours it", () => {
    const redeclared = blocks(CSS)
      .filter((b) => b.decls.has("--accent"))
      .map((b) => b.selector)
      .filter((s) => s !== ":root");
    expect(redeclared).toEqual([]);
  });

  it("carry no chromatic colour outside the six meanings", () => {
    const extra = [...ROOT.entries()]
      .filter(([, v]) => v.startsWith("light-dark("))
      .map(([k]) => k)
      .filter((k) => !k.endsWith("-fill") && !k.endsWith("-border"))
      .filter((k) => {
        const [l, d] = pair(k);
        const sat = (c: Rgba) => Math.max(...c.slice(0, 3)) - Math.min(...c.slice(0, 3));
        return sat(l) > 24 || sat(d) > 24;
      })
      .filter((k) => !(k in CHROMATIC));
    expect(extra).toEqual([]);
  });

  it("keep the AI colour a hue of its own, apart from the accent", () => {
    for (const i of [0, 1] as const) {
      const a = hue(pair("--accent")[i]);
      const ai = hue(pair("--ai")[i]);
      const gap = Math.min(Math.abs(a - ai), 360 - Math.abs(a - ai));
      expect(gap).toBeGreaterThan(60);
    }
  });

  it("hold every text colour to AA on every surface, light and dark", () => {
    const low: string[] = [];
    for (const i of [0, 1] as const) {
      for (const s of SURFACES) {
        const under = pair(s)[i];
        for (const t of TEXTS) {
          const c = contrast(over(pair(t)[i], under), under);
          if (c < 4.5) low.push(`${t} on ${s} (${i ? "dark" : "light"}): ${c.toFixed(2)}`);
        }
        for (const [t, f] of TINTED) {
          const fill = over(pair(f)[i], under);
          const c = contrast(pair(t)[i], fill);
          if (c < 4.5)
            low.push(`${t} on ${f} over ${s} (${i ? "dark" : "light"}): ${c.toFixed(2)}`);
        }
      }
      const on = contrast(pair("--text-on-accent")[i], pair("--accent")[i]);
      if (on < 4.5)
        low.push(`--text-on-accent on --accent (${i ? "dark" : "light"}): ${on.toFixed(2)}`);
    }
    expect(low).toEqual([]);
  });
});
