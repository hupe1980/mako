import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The Desk's own lint plugins (desk/lint/*.grit) run in `biome ci`, which
// `npm run check` runs. Each is held here to the code it must refuse, so a
// plugin that stops matching — a Biome upgrade, a pattern edit — fails.

const desk = resolve(import.meta.dirname, "../..");

/**
 * Biome's verdict, under the Desk's configuration, on `source` written to
 * `path` in a scratch directory — Biome runs no plugin over stdin.
 */
function lint(path: string, source: string): { ok: boolean; out: string } {
  const file = resolve(mkdtempSync(resolve(tmpdir(), "desk-lint-")), path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, source);
  const r = spawnSync(
    resolve(desk, "node_modules/.bin/biome"),
    ["lint", "--colors=off", "--config-path=biome.json", file],
    { cwd: desk, encoding: "utf8" },
  );
  return { ok: r.status === 0, out: `${r.stdout}${r.stderr}` };
}

const BACKTICK = "`";

// Each case starts Biome once; on a loaded machine that takes seconds.
describe("the Desk's lint plugins", { timeout: 30_000 }, () => {
  it("a_raw_html_sink_fails_the_check", () => {
    for (const sink of [
      "export const A = (t: string) => <div dangerouslySetInnerHTML={{ __html: t }} />;",
      "export const A = (el: HTMLElement, t: string) => { el.innerHTML = t; };",
      "export const A = (u: { url: string }) => <a href={u.url}>x</a>;",
      `export const A = (t: string) => <a href={${BACKTICK}https://$\{t}${BACKTICK}}>x</a>;`,
    ]) {
      const r = lint("src/views/probe.tsx", sink);
      expect(r.ok, sink).toBe(false);
      expect(r.out, sink).toContain("No raw HTML sink");
    }
  });

  it("lets a route and a validated link through", () => {
    const r = lint(
      "src/views/probe.tsx",
      `export const A = ({ k, link }: { k: string; link: string }) => <><a href={${BACKTICK}/prozess?key=$\{k}${BACKTICK}}>a</a><a href={link}>b</a></>;\n`,
    );
    expect(r.out).not.toContain("No raw HTML sink");
  });

  it("refuses Number( and parseFloat( outside src/format/", () => {
    const r = lint(
      "src/views/probe.ts",
      "export const x = (a: string) => Number(a) + parseFloat(a);\n",
    );
    expect(r.ok).toBe(false);
    expect(r.out).toContain("No Number( or parseFloat(");
    expect(
      lint("src/format/probe.ts", "export const x = (a: string) => Number(a);\n").out,
    ).not.toContain("No Number(");
  });
});
