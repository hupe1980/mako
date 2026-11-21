// The validator's module and the helpers both pages share: the Validator
// (validator.js) and the landing page's trace (trace.js). Every value reaches
// the page as text — `el` builds no markup from a string.

/** The most a message may be: a market message, not an archive. */
export const MAX = 2 * 1024 * 1024;

/** `el("div", {class: "x"}, "text", child)` — every string becomes a text node. */
export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === false || v == null) continue;
    if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

/** Load mako.wasm from `url`; resolves to `call(request) → answer`, or null without WebAssembly. */
export async function load(url) {
  if (typeof WebAssembly !== "object") return null;
  const module = await WebAssembly.compileStreaming(fetch(url)).catch(async () =>
    WebAssembly.compile(await (await fetch(url)).arrayBuffer()),
  );
  // `time-tz` links wasm-bindgen intrinsics for a system time zone the module
  // never asks for; each import fails loudly if it is ever called.
  const imports = {};
  for (const { module: m, name } of WebAssembly.Module.imports(module)) {
    (imports[m] ??= {})[name] = () => {
      throw new Error(`mako.wasm called ${m}.${name}`);
    };
  }
  const { exports: x } = await WebAssembly.instantiate(module, imports);
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return (request) => {
    const input = enc.encode(JSON.stringify(request));
    const ptr = x.mw_alloc(input.length);
    new Uint8Array(x.memory.buffer, ptr, input.length).set(input);
    const packed = x.mw_call(ptr, input.length);
    x.mw_free(ptr, input.length);
    const outPtr = Number(packed >> 32n);
    const outLen = Number(packed & 0xffffffffn);
    const out = dec.decode(new Uint8Array(x.memory.buffer, outPtr, outLen));
    x.mw_free(outPtr, outLen);
    return JSON.parse(out);
  };
}

/** A file's text: gunzipped when it is gzip, UTF-8 when valid, else ISO 8859-1 (UNOC). */
export async function readFile(file, tooBig) {
  if (file.size > MAX * 4) throw new Error(tooBig);
  let bytes = new Uint8Array(await file.arrayBuffer());
  if (bytes[0] === 0x1f && bytes[1] === 0x8b) {
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
    bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  }
  if (bytes.length > MAX) throw new Error(tooBig);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    let s = "";
    for (const b of bytes) s += String.fromCharCode(b);
    return s;
  }
}

/** On a narrow screen the tab row scrolls; bring the current tab into view. */
export function showCurrentTab() {
  const row = document.querySelector(".vt-tabs");
  const tab = row?.querySelector('[aria-current="page"]');
  if (tab && row.scrollWidth > row.clientWidth) {
    row.scrollLeft = tab.offsetLeft - (row.clientWidth - tab.offsetWidth) / 2;
  }
}
