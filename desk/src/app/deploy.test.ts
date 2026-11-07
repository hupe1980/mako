import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "../..");
const read = (p: string) => readFileSync(resolve(root, p), "utf8");

describe("the Desk image", () => {
  // React Aria injects its own <style>; under `style-src 'self'` it renders
  // only with the per-request nonce. The placeholder in index.html, the
  // sub_filter that fills it and the nonce in the CSP must name one value, or
  // every page loses its styles with no error but a console line.
  it("fills the CSP nonce it admits", () => {
    const html = read("index.html");
    const nginx = read("deploy/nginx.conf");
    const placeholder = /<meta property="csp-nonce" nonce="([^"]+)" content="\1"/.exec(html)?.[1];
    expect(placeholder).toBeTruthy();
    expect(nginx).toContain(`sub_filter "${placeholder}" $request_id;`);
    expect(nginx).toMatch(/style-src 'self' 'nonce-\$request_id'/);
    expect(nginx).toMatch(/script-src 'self';/);
    expect(nginx).not.toMatch(/unsafe-inline/);
  });

  // Every service the Desk reads or writes — each one `src/data/gen` holds a
  // type file for, and the assistant — is a gateway route. One without is a
  // view that works under `just desk` and answers 404 behind the gateway.
  it("routes every service the Desk reaches through the gateway", () => {
    const gateway = read("deploy/gateway.yaml");
    const services = [
      ...readdirSync(resolve(root, "src/data/gen")).map((f) => f.replace(/\.ts$/, "")),
      "agentd",
    ];
    for (const s of services) expect(gateway).toContain(`path: ^/svc/${s}/(.*)$`);
  });

  // A deep link is the app: /freigaben must reach index.html, not a 404.
  it("answers every non-file path with the app", () => {
    expect(read("deploy/nginx.conf")).toContain("try_files $uri /index.html;");
  });
});
