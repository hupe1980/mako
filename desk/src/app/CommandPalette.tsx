import { useNavigate } from "@tanstack/react-router";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import {
  Autocomplete,
  Dialog,
  Header,
  Input,
  Label,
  Menu,
  MenuItem,
  MenuSection,
  Modal,
  ModalOverlay,
  Radio,
  RadioGroup,
  SearchField,
  TextField,
  useFilter,
} from "react-aria-components";
import type { Answer, Bilanz } from "../data/agent";
import { useVerweigert, useVerweigerungen } from "../data/verweigert";
import { AiNote } from "../design/AiNote";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Icon, type IconName } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { Notice } from "../design/Panel";
import type { QuelleId } from "../inbox/model";
import { useQuellen, useVorschlaege } from "../inbox/quellen";
import { VorschlagBilanz } from "../views/Bilanz";
import { monogram, useKundenSuche } from "../views/kundenSuche";
import {
  type Aktion,
  ART,
  aktionen,
  fokussiert,
  type Gefunden,
  gefunden,
  type Herkunft,
  kennungen,
  ort,
  schluessel,
  useHalten,
  vorschlagIds,
  type Werte,
  wartendAuf,
} from "./aktionen";
import { DIENST } from "./dienste";
import { frageMoeglich, frageStellen } from "./frage";
import { PALETTE } from "./keys";
import { ROLLEN, type Rolle, useRolle } from "./rollen";

interface Command {
  id: string;
  label: string;
  icon: IconName;
  hint?: string;
  run: () => void;
  /** Found by a service, so the palette's own text filter must not hide it. */
  direct?: boolean;
  /** Initials shown instead of the icon — a customer, not a command. */
  monogram?: string;
  /** Opens a further step of the palette instead of closing it. */
  bleibt?: boolean;
}

/** Where the palette is: the search, an entry's actions, or one action's parameters. */
type Stufe =
  | { art: "suche" }
  | { art: "aktionen"; g: Gefunden; vomFokus: boolean }
  | { art: "form"; g: Gefunden; a: Aktion; vomFokus: boolean };

const SUCHE: Stufe = { art: "suche" };

/** The sources whose items carry a decision ⌘K can act on. */
const ENTSCHEIDBAR: readonly QuelleId[] = ["freigaben", "dispute", "pruefposten", "geparkt"];
const NICHTS: readonly QuelleId[] = [];

/** What a typed key most likely is — a label only; nothing is validated here. */
export function keyKind(key: string): string {
  if (/^\d{11}$/.test(key)) return "MaLo-ID";
  if (/^DE[0-9A-Z]{31}$/i.test(key)) return "MeLo-ID";
  return "Vorgangsnummer";
}

const ITEM =
  "flex cursor-default items-center gap-3 rounded-[8px] px-3 py-2 text-callout text-text outline-none data-[focused]:bg-accent data-[focused]:text-on-accent";

/**
 * ⌘K: every view and every action, by name. Typing a business key offers to
 * open its process straight away, and finds the open decisions it identifies —
 * a Freigabe, a Disput, a Prüfposten, a geparkte Buchung — whose actions it
 * then lists. Opened on a focused Heute row or the selected Freigabe, it opens
 * on that entry's actions.
 */
export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [input, setInput] = useState("");
  const [stufe, setStufe] = useState<Stufe>(SUCHE);
  const navigate = useNavigate();
  const { rolle, held, wear } = useRolle();
  const { contains } = useFilter({ sensitivity: "base" });

  // Opening reads the entry in focus once: the palette starts on its actions.
  const [warOffen, setWarOffen] = useState(open);
  if (open !== warOffen) {
    setWarOffen(open);
    const f = open ? fokussiert() : null;
    setStufe(f ? { art: "aktionen", g: f, vomFokus: true } : SUCHE);
    if (!open) setInput("");
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (PALETTE.match?.(e)) {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  const close = () => {
    onOpenChange(false);
    setInput("");
    setStufe(SUCHE);
  };

  const typed = input.trim();

  // The open decisions an identifier names, among those Heute reads — the
  // same queries, so a visit to Heute already answered them.
  const staende = useQuellen(rolle, open ? ENTSCHEIDBAR : NICHTS);
  const gesehen = new Set<string>();
  const entscheiden: Command[] = typed
    ? staende
        .flatMap((st) => st.eintraege)
        .flatMap((e) => {
          const g = gefunden(e);
          if (!g || !kennungen(g.ding).includes(typed)) return [];
          const k = schluessel(g.ding);
          if (gesehen.has(k)) return [];
          gesehen.add(k);
          return [
            {
              id: `entscheiden-${k}`,
              label: g.titel,
              icon: "check" as const,
              hint: `${ART[g.ding.art]} · ${g.grund}`,
              direct: true,
              bleibt: true,
              run: () => setStufe({ art: "aktionen", g, vomFokus: false }),
            },
          ];
        })
    : [];

  // The waiting proposal of the entry the palette acts on — read, never asked for.
  const ding = stufe.art === "suche" ? null : stufe.g.ding;
  const wartend = useVorschlaege(ding ? vorschlagIds(ding) : {});
  const vorschlag = ding ? wartendAuf(ding, wartend) : undefined;
  // A business key — MaLo-ID, MeLo-ID, Vorgangsnummer — always has a digit;
  // a name never needs the „Prozess … öffnen" row.
  const direct: Command[] =
    typed.length >= 5 && /^[0-9A-Za-z-]+$/.test(typed) && /\d/.test(typed)
      ? [
          {
            id: "open-key",
            label: `Prozess ${typed} öffnen`,
            icon: "flow",
            hint: keyKind(typed),
            run: () => navigate({ to: "/prozess", search: { key: typed } }),
          },
        ]
      : [];

  // The Lieferant's Anker: customers, searched in vertragd as the person types.
  const suche = useKundenSuche(open ? typed : "");
  const kunden: Command[] =
    rolle === "lieferant" && typed.length >= 3
      ? (suche.data?.treffer ?? []).map((k) => ({
          id: `kunde-${k.id}`,
          label: k.name ?? "Ohne Namen",
          icon: "person" as const,
          monogram: monogram(k.name),
          hint: [
            k.kunden_nr,
            k.lieferstellen === 1 ? "1 Lieferstelle" : `${k.lieferstellen} Lieferstellen`,
          ]
            .filter(Boolean)
            .join(" · "),
          direct: true,
          run: () => navigate({ to: "/kunde/$id", params: { id: k.id } }),
        }))
      : [];

  // On a page with a Fragen panel, anything typed can go to the assistant.
  const fragen: Command[] =
    open && typed.length >= 3 && frageMoeglich()
      ? [
          {
            id: "fragen",
            label: `„${typed}"`,
            icon: "sparkle",
            hint: "Den Assistenten fragen",
            direct: true,
            run: () => frageStellen(typed),
          },
        ]
      : [];

  const views: Command[] = [
    {
      id: "ueberblick",
      label: "Überblick",
      icon: "bars",
      run: () => navigate({ to: "/ueberblick", search: {} }),
    },
    {
      id: "assistent",
      label: "Assistent",
      icon: "sparkle",
      run: () => navigate({ to: "/assistent", search: {} }),
    },
    {
      id: "fristen",
      label: "Fristen überschritten",
      icon: "clock",
      run: () => navigate({ to: "/fristen", search: {} }),
    },
    ...(rolle === "lieferant"
      ? [
          {
            id: "kunden",
            label: "Kunden",
            icon: "person" as const,
            run: () => navigate({ to: "/kunden", search: {} }),
          },
        ]
      : []),
    {
      id: "prozesse",
      label: "Prozesse",
      icon: "flow",
      run: () => navigate({ to: "/prozess", search: {} }),
    },
    ...(import.meta.env.DEV
      ? [
          {
            id: "gallery",
            label: "Gallery",
            icon: "grid" as const,
            hint: "Entwicklung",
            run: () => navigate({ to: "/dev/gallery" }),
          },
        ]
      : []),
  ];

  const rollen: Command[] = held
    .filter((r) => r !== rolle)
    .map((r: Rolle) => ({
      id: `rolle-${r}`,
      label: `Rolle: ${ROLLEN[r].label}`,
      icon: "tray",
      hint: `⌃${ROLLEN[r].key}`,
      run: () => wear(r),
    }));

  const sections: [string, Command[]][] = (
    [
      ["Entscheiden", entscheiden],
      ["Kunden", kunden],
      ["Öffnen", direct],
      ["Assistent", fragen],
      ["Ansichten", views],
      ["Rolle wechseln", rollen],
    ] as [string, Command[]][]
  ).filter(([, items]) => items.length > 0);

  const all = sections.flatMap(([, items]) => items);

  return (
    <ModalOverlay
      isOpen={open}
      onOpenChange={(o) => (o ? onOpenChange(true) : close())}
      isDismissable
      // Past the search, Esc goes back a step; the panels handle it.
      isKeyboardDismissDisabled={stufe.art !== "suche"}
      className="fixed inset-0 z-50 bg-black/20 data-[entering]:animate-in"
    >
      <Modal className="mx-auto mt-[14vh] w-[min(600px,calc(100vw-32px))] outline-none">
        <Dialog
          aria-label="Befehle"
          className="squircle overflow-hidden rounded-panel bg-surface-raised shadow-popover outline-none"
        >
          {stufe.art === "aktionen" && (
            <AktionPanel
              g={stufe.g}
              wartend={vorschlag}
              onWaehlen={(a) => setStufe({ ...stufe, art: "form", a })}
              onZurueck={() => setStufe(SUCHE)}
            />
          )}
          {stufe.art === "form" && (
            <AktionForm
              key={stufe.a.id}
              g={stufe.g}
              a={stufe.a}
              wartend={vorschlag}
              onZurueck={() => setStufe({ art: "aktionen", g: stufe.g, vomFokus: stufe.vomFokus })}
              onGehalten={() => {
                const wohin = stufe.vomFokus ? null : ort(stufe.g.ding);
                close();
                if (wohin) navigate(wohin);
              }}
            />
          )}
          {stufe.art === "suche" && (
            <Autocomplete
              inputValue={input}
              onInputChange={setInput}
              filter={(text, query) => text === "__direct__" || contains(text, query)}
            >
              <SearchField
                aria-label="Befehl oder Geschäftsschlüssel"
                autoFocus
                className="border-b border-separator"
              >
                <div className="flex items-center gap-3 px-4">
                  <Icon name="search" size={18} className="text-text-2" />
                  <Input
                    placeholder="Befehl oder Geschäftsschlüssel …"
                    className="h-14 flex-1 bg-transparent text-[17px] text-text outline-none placeholder:text-text-2 [&::-webkit-search-cancel-button]:hidden"
                  />
                  <Kbd>esc</Kbd>
                </div>
              </SearchField>
              <Menu
                aria-label="Befehle"
                onAction={(id) => {
                  const c = all.find((x) => x.id === id);
                  c?.run();
                  if (!c?.bleibt) close();
                }}
                renderEmptyState={() => (
                  <p className="m-0 px-4 py-6 text-center text-body text-text-2">
                    Kein Befehl passt zu „{input}“.
                  </p>
                )}
                className="max-h-[50vh] overflow-auto p-2 outline-none"
              >
                {sections.map(([title, items]) => (
                  <MenuSection key={title} className="mb-1">
                    <Header className="px-3 pb-1 pt-2 text-caption font-semibold uppercase tracking-wide text-text-2">
                      {title}
                    </Header>
                    {items.map((c) => (
                      <MenuItem
                        key={c.id}
                        id={c.id}
                        textValue={c.id === "open-key" || c.direct ? "__direct__" : c.label}
                        className={ITEM}
                      >
                        {c.monogram ? (
                          <span className="flex size-6 items-center justify-center rounded-full bg-[color-mix(in_srgb,currentColor_14%,transparent)] text-caption font-semibold">
                            {c.monogram}
                          </span>
                        ) : (
                          <Icon name={c.icon} size={16} />
                        )}
                        <span className="flex-1">{c.label}</span>
                        {c.hint && <span className="text-body opacity-70">{c.hint}</span>}
                      </MenuItem>
                    ))}
                  </MenuSection>
                ))}
              </Menu>
            </Autocomplete>
          )}
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** The entry the palette acts on, above its actions or its parameters. */
function Kopf({ g, zusatz }: { g: Gefunden; zusatz?: string }) {
  return (
    <div className="flex items-start gap-3 border-b border-separator px-4 py-3">
      <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-accent">
        <Icon name="check" size={16} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-caption font-semibold tracking-wide text-text-2 uppercase">
          {ART[g.ding.art]}
          {zusatz && <> · {zusatz}</>}
        </span>
        <span
          className={`block truncate text-callout font-medium text-text ${g.kennung ? "font-mono" : ""}`}
        >
          {g.titel}
        </span>
        <span className="block truncate text-body text-text-2">{g.grund}</span>
      </span>
      {g.frist && (
        <span className="flex shrink-0 flex-col items-end gap-1 text-caption text-text-2">
          Frist
          <FristUhr due={g.frist} />
        </span>
      )}
    </div>
  );
}

/** The proposal is for this action: the values it fills in, or null. */
function vorbelegt(a: Aktion, wartend: Answer | undefined): Werte | null {
  const v = wartend?.vorschlag;
  return v ? a.vorbelegen(v) : null;
}

/**
 * An entry's actions, as Raycast's Action Panel lists them: typing filters,
 * ↑↓ move, ↵ runs the marked one (the primary when nothing was moved), ⌘↵ the
 * secondary, esc goes back. An action the service refused this session stands
 * in its words and cannot be chosen.
 */
export function AktionPanel({
  g,
  wartend,
  onWaehlen,
  onZurueck,
  vorschau,
}: {
  g: Gefunden;
  wartend: Answer | undefined;
  onWaehlen: (a: Aktion) => void;
  onZurueck: () => void;
  /** The gallery's still: no focus taken, and a refusal to show. */
  vorschau?: { verweigert?: string };
}) {
  const [filter, setFilter] = useState("");
  const [markiert, setMarkiert] = useState(0);
  const { contains } = useFilter({ sensitivity: "base" });
  const gespeichert = useVerweigerungen();
  const verweigert = (h: Aktion["handlung"]) => vorschau?.verweigert ?? gespeichert(h);
  const liste = useId();
  const alle = aktionen(g.ding);
  const sichtbar = alle.filter((a) => contains(a.verb, filter.trim()));
  const offen = (a: Aktion | undefined) => (a && !verweigert(a.handlung) ? a : undefined);

  function onKeyDown(e: ReactKeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onZurueck();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = sichtbar.length;
      if (n) setMarkiert((m) => (m + (e.key === "ArrowDown" ? 1 : -1) + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const a = offen(
        e.metaKey || e.ctrlKey ? sichtbar.find((x) => x.rang === "sekundaer") : sichtbar[markiert],
      );
      if (a) onWaehlen(a);
    }
  }

  return (
    <div>
      <Kopf g={g} />
      <div className="flex items-center gap-3 border-b border-separator px-4">
        <Icon name="search" size={18} className="text-text-2" />
        <Input
          autoFocus={!vorschau}
          role="combobox"
          aria-label="Aktion filtern"
          aria-expanded
          aria-controls={liste}
          aria-activedescendant={
            sichtbar[markiert] ? `${liste}-${sichtbar[markiert].id}` : undefined
          }
          value={filter}
          onChange={(e) => {
            setFilter(e.target.value);
            setMarkiert(0);
          }}
          onKeyDown={onKeyDown}
          placeholder="Aktion …"
          className="h-12 flex-1 bg-transparent text-[17px] text-text outline-none placeholder:text-text-2"
        />
        <Kbd>esc</Kbd>
      </div>
      <div id={liste} role="listbox" aria-label="Aktionen" className="p-2">
        {sichtbar.map((a, i) => {
          const grund = verweigert(a.handlung);
          const vorgeschlagen = vorbelegt(a, wartend) !== null;
          return (
            // biome-ignore lint/a11y/useKeyWithClickEvents: the combobox above carries the keyboard; a click is the pointer's way to the same choice
            <div
              key={a.id}
              id={`${liste}-${a.id}`}
              role="option"
              // Focus stays in the combobox, which points here (aria-activedescendant).
              tabIndex={-1}
              aria-selected={i === markiert}
              aria-disabled={grund ? true : undefined}
              onClick={() => !grund && onWaehlen(a)}
              onMouseMove={() => setMarkiert(i)}
              className={`flex cursor-default items-center gap-3 rounded-[8px] px-3 py-2 text-callout aria-disabled:opacity-60 ${i === markiert ? "bg-fill-strong" : ""} ${a.destruktiv ? "text-critical" : "text-text"}`}
            >
              <span className="min-w-0 flex-1">
                <span className="block font-medium">{a.verb}</span>
                {grund && <span className="block text-body text-text-2">{grund}</span>}
              </span>
              {vorgeschlagen && (
                <span className="inline-flex h-5 items-center gap-1 rounded-full border border-ai-border bg-ai-fill px-2 text-caption font-semibold text-ai">
                  <Icon name="sparkle" size={11} />
                  Vorschlag
                </span>
              )}
              <Kbd>{a.rang === "primaer" ? "↵" : "⌘↵"}</Kbd>
            </div>
          );
        })}
      </div>
      {sichtbar.length === 0 && (
        <p className="m-0 px-4 pb-4 text-center text-body text-text-2">
          Keine Aktion passt zu „{filter}“.
        </p>
      )}
    </div>
  );
}

const FELD =
  "focus-ring h-9 rounded-control border border-separator bg-surface px-3 text-body text-text outline-none";
const MARKIERT = "border-ai-border bg-ai-fill";

/**
 * One action's parameters, in the palette: the answer code as a choice of the
 * codes its tree publishes, with the wording the service returned, and its one
 * text field. What the waiting proposal filled in carries the assistant's mark
 * and its record until the person changes it — Tab keeps it, typing replaces
 * it. ↵ holds the decision for undo; no dialog asks, the hold is the way back.
 */
export function AktionForm({
  g,
  a,
  wartend,
  onZurueck,
  onGehalten,
  vorschau,
}: {
  g: Gefunden;
  a: Aktion;
  wartend: Answer | undefined;
  onZurueck: () => void;
  onGehalten: () => void;
  /** The gallery's still: no focus taken, the record given, a refusal to show. */
  vorschau?: { verweigert?: string; bilanz?: Bilanz | null };
}) {
  const halte = useHalten();
  const gespeichert = useVerweigert(a.handlung);
  const verweigert = vorschau?.verweigert ?? gespeichert;
  const leer: Werte = { code: null, text: a.feld?.vorgabe ?? "" };
  const [werte, setWerte] = useState<Werte>(leer);
  const [herkunft, setHerkunft] = useState<Herkunft | null>(null);
  const [beruehrt, setBeruehrt] = useState({ code: false, text: false });
  const vor = vorbelegt(a, wartend);
  const gruppe = useRef<HTMLFieldSetElement>(null);

  // The proposal fills in once it is read — unless the person was first.
  useEffect(() => {
    const v = wartend?.vorschlag;
    if (herkunft || !vor || !v || !wartend || beruehrt.code || beruehrt.text) return;
    setHerkunft({ run: wartend.run, v });
    setWerte({ code: vor.code, text: vor.text || leer.text });
  });

  // Focus the first parameter, or the send where there is none — again once
  // the proposal filled it in, which happens only before the person touched it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `herkunft` is the trigger, not a value read
  useEffect(() => {
    if (vorschau) return;
    const g = gruppe.current;
    const ziel =
      g?.querySelector<HTMLElement>("input:checked") ??
      g?.querySelector<HTMLElement>("input") ??
      g?.querySelector<HTMLElement>("button:not([disabled])");
    ziel?.focus();
  }, [vorschau, herkunft]);

  const codeMarke = herkunft !== null && vor?.code != null && !beruehrt.code;
  const textMarke = herkunft !== null && Boolean(vor?.text) && !beruehrt.text;
  // The mark stays while nothing the proposal filled in was changed.
  const marke =
    herkunft !== null && !(beruehrt.code && vor?.code != null) && !(beruehrt.text && vor?.text);
  const bereit = !verweigert && a.bereit(werte);
  const f = g.ding.art === "freigabe" ? g.ding.f : null;

  function halten() {
    if (!bereit) return;
    halte(g.ding, a, werte, { herkunft });
    onGehalten();
  }

  function onKeyDown(e: ReactKeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onZurueck();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      halten();
    }
  }

  return (
    <div>
      <Kopf g={g} zusatz={a.verb} />
      <fieldset
        ref={gruppe}
        aria-label={`Parameter: ${a.verb}`}
        // In the capture phase: a focused code's radio group stops ↵ and esc
        // from bubbling, so the form takes them before the field does.
        onKeyDownCapture={onKeyDown}
        className="m-0 flex min-w-0 flex-col gap-4 border-0 p-4"
      >
        {verweigert && (
          <Notice
            tone="refusal"
            title={`${DIENST[a.handlung.dienst]} lässt Sie hier nicht entscheiden`}
          >
            {verweigert}
          </Notice>
        )}
        {a.id === "freigeben" && f?.art === "gehalten" && f.antwort && (
          <p className="m-0 text-callout text-text">
            Gesendet wird die festgehaltene Antwort:{" "}
            <span className="font-mono">{f.antwort.code}</span>
            {f.antwort.bedeutung && <> — {f.antwort.bedeutung}</>}
          </p>
        )}
        {a.codes && (
          <RadioGroup
            value={werte.code}
            onChange={(c) => {
              setWerte({ ...werte, code: c });
              setBeruehrt({ ...beruehrt, code: true });
            }}
            isDisabled={Boolean(verweigert)}
            data-vorschlag={codeMarke}
            className={`flex flex-col gap-1.5 rounded-[10px] border p-2 ${codeMarke ? MARKIERT : "border-transparent"}`}
          >
            <Label className="flex items-center gap-1.5 text-caption text-text-2">
              {codeMarke && <Icon name="sparkle" size={11} className="text-ai" />}
              Antwortcode aus <span className="font-mono">{a.codes.ebd}</span>
            </Label>
            {a.codes.codes.map((c) => (
              <Radio
                key={c.code}
                value={c.code}
                className="focus-ring flex cursor-default items-start gap-3 rounded-[10px] border border-separator bg-surface px-3 py-2 data-[hovered]:bg-fill data-[selected]:border-accent"
              >
                <span className="font-mono text-body font-semibold text-text">{c.code}</span>
                <span className="text-body text-text-2">{c.bedeutung}</span>
              </Radio>
            ))}
          </RadioGroup>
        )}
        {a.feld && (
          <TextField
            value={werte.text}
            onChange={(t) => {
              setWerte({ ...werte, text: t });
              setBeruehrt({ ...beruehrt, text: true });
            }}
            isRequired={a.feld.pflicht}
            isDisabled={Boolean(verweigert)}
            data-vorschlag={textMarke}
            className="flex flex-col gap-1"
          >
            <Label className="flex items-center gap-1.5 text-caption text-text-2">
              {textMarke && <Icon name="sparkle" size={11} className="text-ai" />}
              {a.feld.label}
            </Label>
            <Input
              type={a.feld.datum ? "date" : "text"}
              maxLength={512}
              // Typing replaces the assistant's text; Tab moves on and keeps it.
              onFocus={(e) => textMarke && e.currentTarget.select()}
              className={`${FELD} ${a.feld.datum ? "w-48" : ""} ${textMarke ? MARKIERT : ""}`}
            />
          </TextField>
        )}
        {marke && wartend?.vorschlag && (
          <AiNote
            text={`${wartend.disclosure} ${wartend.vorschlag.begruendung}`}
            footer={
              <>
                <p className="m-0 text-caption text-text-2">
                  Vom Assistenten vorgeschlagen — Tab übernimmt, Tippen ersetzt.
                </p>
                <VorschlagBilanz bilanz={vorschau?.bilanz} />
              </>
            }
          />
        )}
        <p className="m-0 text-caption text-text-2">{a.was}</p>
        <div className="flex items-center gap-2">
          <span className="flex-1 text-caption text-text-2">
            <Kbd>esc</Kbd> zurück
          </span>
          <Button
            variant={a.destruktiv ? "destructive" : "primary"}
            isDisabled={!bereit}
            onPress={halten}
            className="h-8"
          >
            {a.verb}
            <Kbd>↵</Kbd>
          </Button>
        </div>
      </fieldset>
    </div>
  );
}
