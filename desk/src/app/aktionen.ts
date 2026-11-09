// The decisions the Desk sends, by the kind of entry they decide: a Freigabe,
// a Rechnungsdisput, a Prüfposten, a geparkte Buchung. Each action names its
// verb, its key, its parameters and whether it is the entry's primary or
// secondary action, and sends through the one function its service door has.
// The views and ⌘K both decide through this table, so a decision is one
// request whichever way the person took — a list of known actions, never a
// parser of free text.

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useSyncExternalStore } from "react";
import { type Answer, type Schicksal, schicksal, type Vorschlag } from "../data/agent";
import { neuBuchen, verwerfen } from "../data/buchung";
import { type Entscheidung, entscheiden } from "../data/freigabe";
import type { GeparkteBuchung } from "../data/gen/accountingd";
import type { Pruefposten } from "../data/gen/billingd";
import type { Disput } from "../data/gen/invoicd";
import type { CodeAuswahl, Fakt, Freigabe } from "../data/gen/processd";
import { ablehnungSenden, beilegen, freigeben, geprueft } from "../data/rechnung";
import type { Handlung } from "../data/verweigert";
import type { BusinessDate, Instant } from "../format/brands";
import { faktKoerper } from "../format/fakt";
import { katalog } from "../i18n";
import type { Eintrag } from "../inbox/model";
import type { Wartend } from "../inbox/quellen";
import { faktenVollstaendig } from "./FaktFelder";
import { useHold } from "./Holds";
import { ABLEHNEN, FREIGEBEN, type Shortcut } from "./keys";

/** An entry with an open decision, by its kind. */
export type Entscheidbar =
  | { art: "freigabe"; f: Freigabe }
  | { art: "disput"; d: Disput }
  | { art: "pruefposten"; r: Pruefposten }
  | { art: "buchung"; b: GeparkteBuchung };

/** What each kind is called where it is listed, in the Sprache in force when read. */
export const ART: Record<Entscheidbar["art"], string> = {
  get freigabe() {
    return katalog().aktionen.art.freigabe;
  },
  get disput() {
    return katalog().aktionen.art.disput;
  },
  get pruefposten() {
    return katalog().aktionen.art.pruefposten;
  },
  get buchung() {
    return katalog().aktionen.art.buchung;
  },
};

/** What the person filled in: the answer code, the one text field, and the facts. */
export interface Werte {
  code: string | null;
  /** The Bemerkung, the Grund, or the Buchungsdatum (`YYYY-MM-DD`). */
  text: string;
  /** The facts the action lists, by their body key. */
  fakten?: Record<string, string>;
}

/** The one text parameter an action takes. */
export interface Feld {
  label: string;
  pflicht: boolean;
  /** A business date, `YYYY-MM-DD`, kept as the string typed. */
  datum: boolean;
  /** What it holds before anyone typed: the service's own default, never the assistant's. */
  vorgabe: string;
}

export interface Aktion {
  id: string;
  verb: string;
  /** The key that chooses it in its view. */
  taste: Shortcut;
  /** ↵ runs the primary, ⌘↵ the secondary. */
  rang: "primaer" | "sekundaer";
  /** Refuses, rejects or discards: styled so, never confirmed by a dialog — the hold is the way back. */
  destruktiv: boolean;
  /** The service action whose 403 the Desk remembers (`verweigert.ts`). */
  handlung: Handlung;
  /** The codes its tree publishes, as the service returned them; null where none is stated. */
  codes: CodeAuswahl | null;
  feld: Feld | null;
  /** The facts it states beside the code, as the service listed them; none where absent. */
  fakten?: Fakt[];
  /** What sending it does — said beside the send, since no decision asks for confirmation. */
  was: string;
  /** What the hold's toast says was decided. */
  gehalten: string;
  /** Everything required is filled in. */
  bereit(w: Werte): boolean;
  /** Send it through the service's door. */
  senden(w: Werte, keepalive: boolean): Promise<void>;
  /** The values a waiting proposal fills in, or null where it proposes another action. */
  vorbelegen(v: Vorschlag): Werte | null;
  /** The values sent are the proposal's — what agentd records as `sent`, else `edited`. */
  stimmt(v: Vorschlag, w: Werte): boolean;
}

const LEER: Werte = { code: null, text: "" };
const ISO_DATUM = /^\d{4}-\d{2}-\d{2}$/;

/** A code only from the tree's published list: anything else is not this field's to hold. */
function codeAus(codes: CodeAuswahl | null, code: string | null): string | null {
  return codes && code && codes.codes.some((c) => c.code === code) ? code : null;
}

/** The optional note, named in the Sprache in force when the actions are listed. */
function bemerkung(label = katalog().aktionen.bemerkung): Feld {
  return { label, pflicht: false, datum: false, vorgabe: "" };
}

function freigabeAktionen(f: Freigabe): Aktion[] {
  const k = katalog().aktionen.freigabe;
  const PROCESSD: Handlung = { dienst: "processd", aktion: "entscheiden" };
  const eine = (
    id: "freigeben" | "ablehnen",
    codes: CodeAuswahl | null,
    fakten: Fakt[],
    rang: Aktion["rang"],
  ): Aktion => ({
    id,
    verb: id === "freigeben" ? k.freigeben : k.ablehnen,
    taste: id === "freigeben" ? FREIGEBEN : ABLEHNEN,
    rang,
    destruktiv: id === "ablehnen",
    handlung: PROCESSD,
    codes,
    feld: bemerkung(),
    fakten,
    was: k.was,
    gehalten: id === "freigeben" ? k.erteilt : k.abgelehnt,
    bereit: (w) =>
      (!codes || codeAus(codes, w.code) !== null) && faktenVollstaendig(fakten, w.fakten ?? {}),
    senden: (w, keepalive) => {
      const e: Entscheidung = { aktion: id };
      const code = codeAus(codes, w.code);
      if (codes && code) {
        e.antwort_code = code;
        e.antwort_tree = codes.ebd;
      }
      if (w.text.trim()) e.bemerkung = w.text.trim();
      return entscheiden(f.id, { ...e, ...faktKoerper(fakten, w.fakten ?? {}) }, keepalive);
    },
    vorbelegen: (v) => (v.aktion === id ? { code: codeAus(codes, v.antwortcode), text: "" } : null),
    stimmt: (v, w) => v.aktion === id && (v.antwortcode ?? null) === (w.code ?? null),
  });
  // processd says which actions the entry offers; one it does not is not listed.
  const alle: Aktion[] = [];
  if (f.kann_freigeben)
    alle.push(eine("freigeben", f.codes_freigeben, f.fakten_freigeben, "primaer"));
  if (f.kann_ablehnen)
    alle.push(
      eine("ablehnen", f.codes_ablehnen, f.fakten_ablehnen, alle.length ? "sekundaer" : "primaer"),
    );
  return alle;
}

function disputAktionen(d: Disput): Aktion[] {
  const k = katalog().aktionen.disput;
  const INVOICD: Handlung = { dienst: "invoicd", aktion: "entscheiden" };
  const beilegenA: Aktion = {
    id: "beilegen",
    verb: k.beilegen,
    taste: FREIGEBEN,
    rang: "primaer",
    destruktiv: false,
    handlung: INVOICD,
    codes: null,
    feld: bemerkung(k.bemerkung),
    was: k.beilegenWas,
    gehalten: k.beigelegt,
    bereit: () => true,
    senden: (w, keepalive) => beilegen(d.id, w.text, keepalive),
    // The Begründung stays the model's: the Bemerkung is the person's own words.
    vorbelegen: (v) => (v.aktion === "beilegen" ? LEER : null),
    stimmt: (v) => v.aktion === "beilegen",
  };
  if (d.abgesendet) return [beilegenA];
  return [
    beilegenA,
    {
      id: "ablehnung",
      verb: k.ablehnung,
      taste: ABLEHNEN,
      rang: "sekundaer",
      destruktiv: true,
      handlung: INVOICD,
      codes: null,
      feld: null,
      was: k.ablehnungWas,
      gehalten: k.gesendet,
      bereit: () => true,
      senden: (_w, keepalive) => ablehnungSenden(d.id, keepalive),
      vorbelegen: (v) => (v.aktion === "aufrechterhalten" ? LEER : null),
      stimmt: (v) => v.aktion === "aufrechterhalten",
    },
  ];
}

function pruefpostenAktionen(r: Pruefposten): Aktion[] {
  const k = katalog().aktionen.pruefposten;
  const gehalten = r.risk_band === "HELD";
  // The assistant proposes nothing here: the risk gate's findings are the evidence.
  const nie = () => null;
  return [
    gehalten
      ? {
          id: "freigeben",
          verb: k.freigeben,
          taste: FREIGEBEN,
          rang: "primaer",
          destruktiv: false,
          handlung: { dienst: "billingd", aktion: "freigeben" },
          codes: null,
          feld: null,
          was: k.freigebenWas,
          gehalten: k.freigegeben,
          bereit: () => true,
          senden: (_w, keepalive) => freigeben(r.id, keepalive),
          vorbelegen: nie,
          stimmt: () => false,
        }
      : {
          id: "geprueft",
          verb: k.geprueft,
          taste: FREIGEBEN,
          rang: "primaer",
          destruktiv: false,
          handlung: { dienst: "billingd", aktion: "pruefen" },
          codes: null,
          feld: null,
          was: k.gepruefWas,
          gehalten: k.vermerkt,
          bereit: () => true,
          senden: (_w, keepalive) => geprueft(r.id, keepalive),
          vorbelegen: nie,
          stimmt: () => false,
        },
  ];
}

function buchungAktionen(b: GeparkteBuchung): Aktion[] {
  const k = katalog().aktionen.buchung;
  const BUCHEN: Handlung = { dienst: "accountingd", aktion: "buchen" };
  return [
    {
      id: "buchen",
      verb: k.buchen,
      taste: FREIGEBEN,
      rang: "primaer",
      destruktiv: false,
      handlung: BUCHEN,
      codes: null,
      // The first open day the service names; the assistant's date replaces it only marked.
      feld: {
        label: k.datum,
        pflicht: true,
        datum: true,
        vorgabe: b.vorschlag_datum ?? "",
      },
      was: k.buchenWas,
      gehalten: k.gebucht,
      bereit: (w) => ISO_DATUM.test(w.text),
      senden: (w, keepalive) => neuBuchen(b.key, w.text as BusinessDate, keepalive),
      vorbelegen: (v) =>
        v.aktion === "neu_buchen" && v.buchungsdatum && ISO_DATUM.test(v.buchungsdatum)
          ? { code: null, text: v.buchungsdatum }
          : null,
      stimmt: (v, w) => v.aktion === "neu_buchen" && w.text === v.buchungsdatum,
    },
    {
      id: "verwerfen",
      verb: k.verwerfen,
      taste: ABLEHNEN,
      rang: "sekundaer",
      destruktiv: true,
      handlung: BUCHEN,
      codes: null,
      feld: {
        label: k.grund,
        pflicht: true,
        datum: false,
        vorgabe: "",
      },
      was: k.verwerfenWas,
      gehalten: k.verworfen,
      bereit: (w) => w.text.trim().length > 0,
      senden: (w, keepalive) => verwerfen(b.key, w.text, keepalive),
      vorbelegen: (v) =>
        v.aktion === "verwerfen" && v.grund?.trim() ? { code: null, text: v.grund } : null,
      stimmt: (v, w) => v.aktion === "verwerfen" && w.text.trim() === v.grund?.trim(),
    },
  ];
}

/** Every action an entry offers, primary first. */
export function aktionen(e: Entscheidbar): Aktion[] {
  switch (e.art) {
    case "freigabe":
      return freigabeAktionen(e.f);
    case "disput":
      return disputAktionen(e.d);
    case "pruefposten":
      return pruefpostenAktionen(e.r);
    case "buchung":
      return buchungAktionen(e.b);
  }
}

/** One action of an entry by its id — what a view's form names. */
export function aktion(e: Entscheidbar, id: string): Aktion | undefined {
  return aktionen(e).find((a) => a.id === id);
}

/** The action a view key chooses (A, R), among those the entry offers. */
export function perTaste(e: Entscheidbar, ev: KeyboardEvent): Aktion | undefined {
  return aktionen(e).find((a) => a.taste.match?.(ev));
}

/** Where a proposal came from, so sending it can say whether it was changed. */
export interface Herkunft {
  run: string;
  v: Vorschlag;
}

/**
 * Send an action, then record what became of the proposal it was filled
 * from — the assistant's quality signal, which never blocks the decision.
 */
export async function ausfuehren(
  a: Aktion,
  w: Werte,
  o: { herkunft?: Herkunft | null; keepalive?: boolean } = {},
): Promise<void> {
  const keepalive = o.keepalive ?? false;
  await a.senden(w, keepalive);
  if (o.herkunft) {
    const was: Schicksal = a.stimmt(o.herkunft.v, w) ? "sent" : "edited";
    await schicksal(o.herkunft.run, was, keepalive).catch(() => undefined);
  }
}

/** What becomes of a held decision, for the view that holds it. */
export interface Ausgang {
  herkunft?: Herkunft | null;
  /** The service took it. */
  gesendet?: () => void;
  /** The person took it back within the hold: nothing was sent. */
  zurueck?: () => void;
  /** The service refused it; its words are in `useFehlschlag(schluessel(e))`. */
  fehlgeschlagen?: (error: unknown) => void;
}

/**
 * Hold a decision for undo, then send it — the one way a decision leaves the
 * Desk, in its view exactly as in ⌘K: no confirmation, five seconds with the
 * ring on its entry and ⌘Z, then the registry's request. A refusal is kept
 * under the entry's key, where the view's form and the entry's ring read it.
 */
export function useHalten() {
  const hold = useHold();
  const client = useQueryClient();
  return useCallback(
    (e: Entscheidbar, a: Aktion, w: Werte, o: Ausgang = {}) => {
      const key = schluessel(e);
      const herkunft = o.herkunft ?? null;
      fehlschlagen(key, null);
      return hold(
        key,
        a.gehalten,
        async (keepalive) => {
          try {
            await ausfuehren(a, w, { herkunft, keepalive });
            o.gesendet?.();
            void client.invalidateQueries();
          } catch (error) {
            fehlschlagen(key, { dienst: a.handlung.dienst, error });
            o.fehlgeschlagen?.(error);
          }
        },
        () => o.zurueck?.(),
      );
    },
    [hold, client],
  );
}

/** One decision per entry: the key its hold runs under, in the views and in ⌘K. */
export function schluessel(e: Entscheidbar): string {
  switch (e.art) {
    case "freigabe":
      return `freigabe/${e.f.id}`;
    case "disput":
      return `disput/${e.d.id}`;
    case "pruefposten":
      return `pruefposten/${e.r.id}`;
    case "buchung":
      return `buchung/${e.b.key}`;
  }
}

/** The identifiers ⌘K finds an entry by: its own id, its MaLo, its process. */
export function kennungen(e: Entscheidbar): string[] {
  switch (e.art) {
    case "freigabe":
      return [e.f.id, e.f.process_id, ...(e.f.malo_id ? [e.f.malo_id] : [])];
    case "disput":
      return [e.d.id, e.d.process_id, ...(e.d.rechnungsnummer ? [e.d.rechnungsnummer] : [])];
    case "pruefposten":
      return [e.r.id, e.r.malo_id, e.r.rechnungsnummer];
    case "buchung":
      return [e.b.key, e.b.malo_id, ...(e.b.beleg ? [e.b.beleg] : [])];
  }
}

/** The entry's open decision, where a Heute item carries one. */
export function entscheidbar(e: Eintrag): Entscheidbar | null {
  if (e.freigabe) return { art: "freigabe", f: e.freigabe };
  if (e.disput) return { art: "disput", d: e.disput };
  if (e.pruefposten) return { art: "pruefposten", r: e.pruefposten };
  if (e.geparkt) return { art: "buchung", b: e.geparkt };
  return null;
}

/** The ids `/vorschlaege` is asked for; a Prüfposten never has a proposal. */
export function vorschlagIds(e: Entscheidbar): Partial<Record<keyof Wartend, string[]>> {
  switch (e.art) {
    case "freigabe":
      return { freigabe: [e.f.id] };
    case "disput":
      return { disput: [e.d.id] };
    case "buchung":
      return { buchung: [e.b.key] };
    case "pruefposten":
      return {};
  }
}

/** The proposal waiting on the entry, from one `/vorschlaege` read. */
export function wartendAuf(e: Entscheidbar, w: Wartend): Answer | undefined {
  switch (e.art) {
    case "freigabe":
      return w.freigabe.get(e.f.id);
    case "disput":
      return w.disput.get(e.d.id);
    case "buchung":
      return w.buchung.get(e.b.key);
    case "pruefposten":
      return undefined;
  }
}

/** The entry as ⌘K lists it and opens on. */
export interface Gefunden {
  titel: string;
  /** Whether `titel` is an identifier, set in mono. */
  kennung: boolean;
  grund: string;
  frist: Instant | null;
  ding: Entscheidbar;
}

export function gefunden(e: Eintrag): Gefunden | null {
  const ding = entscheidbar(e);
  return ding ? { titel: e.titel, kennung: e.kennung, grund: e.grund, frist: e.frist, ding } : null;
}

/** Where the palette closes onto after a decision found by its identifier. */
export function ort(
  e: Entscheidbar,
): { to: "/freigaben"; search: { id: string } } | { to: "/heute" } {
  return e.art === "freigabe" ? { to: "/freigaben", search: { id: e.f.id } } : { to: "/heute" };
}

// The entry the person has in focus — a Heute row, the selected Freigabe —
// which ⌘K opens on. Set by the view that shows it, cleared when it leaves.
let imFokus: Gefunden | null = null;

export function fokussieren(g: Gefunden | null): void {
  imFokus = g;
}

export function fokussiert(): Gefunden | null {
  return imFokus;
}

// A held decision its service then refused: its words, shown on the entry and
// in its view's form, until the next decision on it.
interface Fehlschlag {
  dienst: Handlung["dienst"];
  error: unknown;
}

let fehlschlaege: Record<string, Fehlschlag> = {};
const zuhoerer = new Set<() => void>();

export function fehlschlagen(key: string, f: Fehlschlag | null): void {
  const { [key]: _, ...rest } = fehlschlaege;
  fehlschlaege = f ? { ...rest, [key]: f } : rest;
  for (const l of zuhoerer) l();
}

export function useFehlschlag(key: string): Fehlschlag | null {
  return useSyncExternalStore(
    (l) => {
      zuhoerer.add(l);
      return () => zuhoerer.delete(l);
    },
    () => fehlschlaege[key] ?? null,
  );
}
