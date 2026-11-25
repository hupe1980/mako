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
  // view that works under `just dev` and answers 404 behind the gateway.
  it("routes every service the Desk reaches through the gateway", () => {
    const gateway = read("deploy/gateway.yaml");
    const services = [
      ...readdirSync(resolve(root, "src/data/gen")).map((f) => f.replace(/\.ts$/, "")),
      "agentd",
      "outputd",
    ];
    for (const s of services) expect(gateway).toContain(`path: ^/svc/${s}/(`);
  });

  // The gateway forwards what the Desk calls — `/mcp` and the named write
  // routes — and nothing else: a person's token behind the gateway reaches no
  // service's `/admin`, `/metrics` or any route the Desk does not use. A path
  // no service route matches falls to the Desk's own upstream, which answers
  // with the app, never with the service.
  const routes = () => {
    const gateway = read("deploy/gateway.yaml");
    return [...gateway.matchAll(/- id: ([a-z]+)\n\s+path: (\^\/svc\/[^\n]+)/g)].map((m) => ({
      service: m[1] ?? "",
      // oauth2-proxy runs Go's RE2; these patterns are its common subset.
      pfad: new RegExp(m[2] ?? ""),
    }));
  };
  const forwarded = (path: string) => routes().some((r) => r.pfad.test(path));

  it("forwards the Desk's MCP reads and named write routes", () => {
    for (const path of [
      "/svc/makod/mcp",
      "/svc/makod/api/v1/openapi.json",
      "/svc/marktd/api/v1/openapi.json",
      "/svc/marktd/api/v1/malos/51238696012",
      "/svc/marktd/api/v1/malos/51238696012/lokationsbuendel",
      "/svc/marktd/api/v1/versorgung/51238696012/history",
      "/svc/edmd/api/v1/coverage/51238696012",
      "/svc/edmd/api/v1/timeseries/51238696012",
      "/svc/accountingd/api/v1/accounts/51238696012/balance",
      "/svc/accountingd/api/v1/accounts/51238696012/ledger",
      "/svc/accountingd/api/v1/dunning",
      "/svc/accountingd/api/v1/offene-posten",
      "/svc/accountingd/api/v1/payments/match",
      "/svc/accountingd/api/v1/payments/unmatched",
      "/svc/accountingd/api/v1/payments/pay-1/assign",
      "/svc/accountingd/api/v1/credit-payouts",
      "/svc/accountingd/api/v1/credit-payouts/po-1/paid",
      "/svc/accountingd/api/v1/eeg/payouts",
      "/svc/edmd/api/v1/webhooks/endpoints",
      "/svc/processd/api/v1/webhooks/endpoints/ep-1/deliveries",
      "/svc/marktd/api/v1/webhooks/endpoints/ep-1/rotate-secret",
      "/svc/billingd/api/v1/webhooks/deliveries/d-1/replay",
      "/svc/productd/api/v1/webhooks/endpoints/ep-1/test",
      "/svc/productd/api/v1/events",
      "/svc/einsd/api/v1/anlagen/TR-1",
      "/svc/einsd/api/v1/anlagen/ohne-mastr",
      "/svc/einsd/api/v1/anlagen/foerderung-auslaufend",
      "/svc/einsd/api/v1/anlagen/TR-1/settlements",
      "/svc/einsd/api/v1/anlagen/TR-1/settlements/2026/10/erklaerung",
      "/svc/einsd/api/v1/anlagen/unabgerechnet/2026/10",
      "/svc/marktd/api/v1/lokationsbuendelstrukturen",
      "/svc/edmd/api/v1/quality/51238696012",
      "/svc/edmd/api/v1/corrections/51238696012",
      "/svc/makod/api/v1/probe/gpke.lieferbeginn.anmelden",
      "/svc/processd/mcp",
      "/svc/processd/api/v1/queue/q-1/approve",
      "/svc/processd/api/v1/queue/q-1/reject",
      "/svc/processd/api/v1/counterparties/9800357000005/NB",
      "/svc/processd/api/v1/klaerfaelle/k-1/dispatch",
      "/svc/processd/api/v1/klaerfaelle/k-1/resolve",
      "/svc/processd/api/v1/klaerfaelle/k-1/assign",
      "/svc/vertragd/mcp",
      "/svc/marktd/mcp",
      "/svc/edmd/mcp",
      "/svc/netzbilanzd/mcp",
      "/svc/invoicd/mcp",
      "/svc/invoicd/api/v1/receipts/r-1/resolve-dispute",
      "/svc/invoicd/api/v1/receipts/r-1/dispatch-answer",
      "/svc/invoicd/api/v1/receipts/r-1/feststellungen",
      "/svc/invoicd/api/v1/leistungsnachweise",
      "/svc/billingd/mcp",
      "/svc/outputd/api/v1/openapi.json",
      "/svc/outputd/api/v1/documents/by-subject/INVOICE",
      "/svc/billingd/api/v1/billing/b-1/release",
      "/svc/billingd/api/v1/billing/b-1/reviewed",
      "/svc/accountingd/mcp",
      "/svc/accountingd/api/v1/bookings/k-1/rebook",
      "/svc/accountingd/api/v1/bookings/k-1/discard",
      "/svc/agentd/api/v1/runs",
      "/svc/agentd/api/v1/runs/r-1/antwort",
      "/svc/agentd/api/v1/runs/r-1/schritte",
      "/svc/agentd/api/v1/runs/r-1/stopp",
      "/svc/agentd/api/v1/runs/r-1/bewertung",
      "/svc/agentd/api/v1/vorschlaege",
      "/svc/agentd/api/v1/agents/catalog",
      "/svc/agentd/api/v1/hinweise",
    ])
      expect(forwarded(path), path).toBe(true);
  });

  it("routes shadow reads and resolutions only when an isolated upstream is configured", () => {
    const template = read("deploy/gateway.yaml");
    const optional =
      /# - id: shadowreview\n\s+#\s+path: (.+)\n\s+#\s+rewriteTarget: .+\n\s+#\s+uri: (.+)/.exec(
        template,
      );
    expect(optional?.[2]).toBe("http://processd-review:8580");
    const isolated = new RegExp(optional?.[1] ?? "(?!)");
    for (const path of [
      "/svc/processd/api/v1/shadow/review",
      "/svc/processd/api/v1/shadow/review/row/resolve",
    ]) {
      expect(forwarded(path)).toBe(false);
      expect(isolated.test(path)).toBe(true);
    }
    expect(isolated.test("/svc/processd/api/v1/queue/q-1/approve")).toBe(false);
  });

  it("forwards no admin or metrics path", () => {
    const services = routes().map((r) => r.service);
    expect(services.length).toBeGreaterThan(5);
    expect(forwarded("/svc/outputd/api/v1/documents/by-subject/MAHNUNG/R-1")).toBe(false);
    expect(forwarded("/svc/outputd/api/v1/documents/d-1/content")).toBe(false);
    for (const s of services)
      for (const path of [
        `/svc/${s}/admin/einrichtung`,
        `/svc/${s}/metrics`,
        `/svc/${s}/health/ready`,
        `/svc/${s}/api/v1/policy`,
        `/svc/${s}/mcp/../admin`,
        `/svc/${s}/api/v1/queue/q-1/approve/../../../admin`,
      ])
        expect(forwarded(path), path).toBe(false);
  });

  // A deep link is the app: /freigaben must reach index.html, not a 404.
  it("ships the session cookie SameSite=Strict, Secure and HttpOnly", () => {
    const cfg = read("deploy/oauth2-proxy.cfg");
    expect(cfg).toMatch(/^cookie_samesite\s*=\s*"strict"$/m);
    expect(cfg).toMatch(/^cookie_secure\s*=\s*true$/m);
    expect(cfg).toMatch(/^cookie_httponly\s*=\s*true$/m);
    expect(read("deploy/gateway.yaml")).toContain("--config=oauth2-proxy.cfg");
  });

  it("cannot be framed, so no other site overlays a decision (clickjacking)", () => {
    const nginx = read("deploy/nginx.conf");
    expect(nginx).toMatch(/Content-Security-Policy "[^"]*frame-ancestors 'none'[^"]*" always;/);
  });

  it("answers every non-file path with the app", () => {
    expect(read("deploy/nginx.conf")).toContain("try_files $uri /index.html;");
  });
});

describe("the demo edge (deploy/demo/desk/nginx.conf)", () => {
  const demo = () => read("../deploy/demo/desk/nginx.conf");

  // `just demo` serves the same bundle, so the same nonce must reach React
  // Aria's <style>, under the same CSP.
  it("fills the CSP nonce it admits", () => {
    const placeholder = /<meta property="csp-nonce" nonce="([^"]+)" content="\1"/.exec(
      read("index.html"),
    )?.[1];
    expect(demo()).toContain(`sub_filter "${placeholder}" $request_id;`);
    expect(demo()).toMatch(/style-src 'self' 'nonce-\$request_id'/);
    expect(demo()).not.toMatch(/unsafe-inline/);
  });

  // Every service the production gateway routes is a door of the demo edge,
  // and each one but makod carries the persona's token.
  it("routes every service the gateway routes, with the persona's token", () => {
    const gateway = read("deploy/gateway.yaml");
    const services = [...gateway.matchAll(/path: \^\/svc\/([a-z]+)\//g)].map((m) => m[1]);
    expect(services.length).toBeGreaterThan(5);
    for (const s of services) {
      const location =
        s === "outputd"
          ? String.raw`location ~ ^/svc/outputd/(api/v1/openapi\.json|api/v1/documents/by-subject/INVOICE)$ {`
          : `location /svc/${s}/ {`;
      const block = demo().split(location)[1]?.split("}")[0];
      expect(block, s).toBeTruthy();
      expect(block).toContain(s === "makod" ? "Bearer dev-makod-key" : "$mako_authorization");
    }
  });

  // The link on a letter opens through the one published port, and the public
  // page never sees a persona token.
  it("serves the public Nachrechnen page from portald without a token", () => {
    const block = /location \/nachrechnen\/ \{[^}]*\}/.exec(demo())?.[0];
    expect(block).toContain("portald:9480");
    expect(block).toContain('proxy_set_header Authorization "";');
    expect(block).not.toContain("$mako_authorization");
  });
});
