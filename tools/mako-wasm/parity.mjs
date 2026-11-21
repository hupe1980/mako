// The parity check's WebAssembly half: run the validator's module over the
// committed requests and compare every answer, byte for byte, with what the
// native build gave (tests/parity). Also holds the size budget, and the two
// page rules a message's content depends on: the script renders text only,
// and the page's Content-Security-Policy lets no connection leave the origin.
//
//   node tools/mako-wasm/parity.mjs site/static/validator/mako.wasm
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const [wasmPath] = process.argv.slice(2);
const BUDGET = 1.5 * 1024 * 1024;
const page = (f) => readFileSync(new URL(`../../site/${f}`, import.meta.url), "utf8");
const script = page("static/validator/validator.js");
const markup = /\b(innerHTML|outerHTML|insertAdjacentHTML|document\.write|eval\s*\(|new Function)\b/.exec(script);
if (markup) {
  console.error(`✗ validator.js uses ${markup[1]}: message content must reach the page as text only`);
  process.exit(1);
}
const csp = /http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(page("templates/validator.html"))?.[1] ?? "";
if (!/(^|;)\s*connect-src 'self'\s*(;|$)/.test(csp) || !/(^|;)\s*default-src 'self'\s*(;|$)/.test(csp)) {
  console.error("✗ the validator's Content-Security-Policy must keep default-src and connect-src to 'self'");
  process.exit(1);
}
const bytes = readFileSync(wasmPath);
const gz = gzipSync(bytes, { level: 9 }).length;
if (gz > BUDGET) {
  console.error(`✗ ${wasmPath}: ${gz} bytes gzip, over the 1.5 MB budget`);
  process.exit(1);
}
// `time-tz` links wasm-bindgen's intrinsics on wasm32 for a system time zone
// the module never asks for (mako-fristen names Europe/Berlin). Each import
// is a stub that fails loudly if it is ever called; the page does the same.
const module = await WebAssembly.compile(bytes);
const imports = {};
for (const { module: m, name } of WebAssembly.Module.imports(module)) {
  (imports[m] ??= {})[name] = () => {
    throw new Error(`mako.wasm called ${m}.${name}`);
  };
}
const instance = await WebAssembly.instantiate(module, imports);
const { memory, mw_alloc, mw_free, mw_call } = instance.exports;
const enc = new TextEncoder();
const dec = new TextDecoder();

function call(request) {
  const input = enc.encode(request);
  const ptr = mw_alloc(input.length);
  new Uint8Array(memory.buffer, ptr, input.length).set(input);
  const packed = mw_call(ptr, input.length);
  mw_free(ptr, input.length);
  const outPtr = Number(packed >> 32n);
  const outLen = Number(packed & 0xffffffffn);
  const text = dec.decode(new Uint8Array(memory.buffer, outPtr, outLen));
  mw_free(outPtr, outLen);
  return text;
}

const dir = new URL("tests/parity/", import.meta.url);
const lines = (f) => readFileSync(new URL(f, dir), "utf8").split("\n").filter(Boolean);
const requests = lines("requests.jsonl");
const answers = lines("answers.jsonl");
let failed = 0;
requests.forEach((r, i) => {
  if (call(r) !== answers[i]) {
    console.error(`✗ request ${i + 1} answers differently in WebAssembly`);
    failed++;
  }
});
if (failed) process.exit(1);
console.log(`✓ ${requests.length} answers match the native build · ${(gz / 1024).toFixed(0)} KB gzip`);
