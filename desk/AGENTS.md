# AGENTS.md — desk/

mako Desk, the operator's web UI: React 19, TypeScript (strict), Vite, Tailwind
4, React Aria Components, TanStack Router/Query. It reads the services' MCP
tools — the surface agents are granted — and writes through their REST routes.

## Run it

```bash
just desk          # makod on its dev config + the Desk with hot reload, browser on :5173
just desk-gen      # refresh services/*/mcp.snapshot.json and regenerate src/data/gen
just desk-check    # generated types current, tsc, biome, vitest — part of `just ci`
just desk-vergleich  # Einrichtung screenshots against desk/vergleich/; --bless after a deliberate change
```

Node comes from `.node-version`; npm ships with it. `npm ci` runs no install
script (`allowScripts` in `package.json`).

## Rules

- **The Desk computes nothing that decides.** A Frist, amount, Werktag, verdict
  or permission comes from a service; the Desk renders it. Only `src/format/`
  turns an instant or an amount into text, and `cargo xtask check-desk` refuses
  `new Date(`, `Date.now(`, `Number(`, `parseFloat(` … anywhere else.
- **Name no service.** An operator knows the Marktkommunikation, not
  `makod`: captions, errors and source chips name services through
  `src/app/dienste.ts`, never by their crate name.
- **The assistant is agentd's run API**, the one REST read (`src/data/agent.ts`):
  a question starts a run, its answer arrives sentence by sentence with the
  check agentd made — the Desk shows `belegt`, it never decides it.
- **A view reads typed tools only.** `call(service, tool, args)` accepts the
  tools in `src/data/gen/<service>.ts`, generated from the service's
  `mcp.snapshot.json`; `<service>Untyped` lists the rest. To read a new tool,
  give it an output type in the service (`#[tool(output_schema = …)]` with a
  `CallToolResult::structured` answer), then `just desk-gen`.
- **Never edit `src/data/gen/`.** `npm run gen -- --check` fails on a hand edit.
- **Business dates stay strings.** `BusinessDate`, `Instant` and `Decimal` are
  branded strings; format them with `src/format/`, never through `Date` or
  `number`. Unit tests run with `TZ=America/New_York`, so a date that is only
  right in Germany fails.
- **No request per row; the service orders, filters and counts.** A tenant
  holds up to two million MaLos. A list is the service's page (keyset
  `nach`/`weiter`, a `gesamt` or `saturated` beside it), a page's names come
  from one batch call (vertragd `kunden_zu_malos`), and the Desk never sorts
  or counts a page as if it were the whole list. The view tests count the
  calls a page costs.
- **One Rolle at a time.** Every query key starts with the Rolle; switching
  clears the cache (`src/app/rollen.tsx`). No view combines two Rollen's data.
- **Agent text keeps its disclosure.** Render a model's text through `AiNote`,
  with agentd's `AI-generated.` sentence first, as text.
- **Every word on screen comes from the catalogue** (`src/i18n/`). Add the
  German string to `de.ts` and its English twin to `en.ts` — `en` is typed as
  `de`'s shape, so a missing key is a type error — and read it with
  `const t = useT()` (`katalog()` outside a component). A sentence with a value
  in it is a function (`zeile: (pid, zustand) => …`), so word order is the
  language's. The market's vocabulary (MaLo, Frist, PID, APERAK, Lieferbeginn,
  Klärfall, Werktag, Antwortcode, process names) stays German in `en.ts` too.
  `cargo xtask check-desk` refuses JSX text and a literal `aria-label`,
  `title`, `placeholder`, `label`, `textValue` or `alt` in any `.tsx`, with no
  file exempt, and in any `.ts` module a literal text property (`label:`,
  `titel:`, `was:`, `verb:`, …) or a code table naming a value in words
  (`STORNO: "Storno"`); `src/i18n/i18n.test.tsx` renders the views under a marking
  catalogue and refuses any unmarked word.
- **A list that opens entries is a `ListDetail`** (`src/design/ListDetail.tsx`)
  of `ListRow`s: ↑ ↓ select, ↵ opens into the detail, esc returns to the row,
  the selection in the address; one pane at a phone's width.
- **Design tokens are the only place a colour, size, radius or duration is
  written** (`src/design/tokens.css`). A new control appears in
  `/dev/gallery` in every state, in light and dark.
- **Dependencies are few on purpose.** A new runtime package needs a reason
  in the pull request, and its licence must already be in `deny.toml`.
