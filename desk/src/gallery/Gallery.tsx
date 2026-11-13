import { type ReactNode, useState } from "react";
import { GridList } from "react-aria-components";
import { LaufPill } from "../app/LaufPills";
import type { Lauf } from "../app/laeufe";
import { ROLLEN, type Rolle } from "../app/rollen";
import type { Answer, Bilanz, Schritt, Vorschlag } from "../data/agent";
import type { GeparkteBuchung } from "../data/gen/accountingd";
import { Abgelehnt } from "../data/rechnung";
import { AiNote } from "../design/AiNote";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { Angabe, ListDetail } from "../design/ListDetail";
import { ListRow } from "../design/ListRow";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SearchField } from "../design/SearchField";
import { SourceChip } from "../design/SourceChip";
import { Table } from "../design/Table";
import { Tile } from "../design/Tile";
import type { BusinessDate, Instant } from "../format/brands";
import { epochMs } from "../format/date";
import { DateiAnsicht } from "../views/Befragung";
import { GeparktEntscheiden } from "../views/Buchungen";
import { AmpelZeichen, BefundZeile, DienstChip } from "../views/Einrichtung";
import { SpurAnsicht } from "../views/Spur";
import { WarumAntwort } from "../views/Warum";
import { AktionenVorschau } from "./Aktionen";

// Every control in every state, in both appearances side by side: the Desk's
// visual regression fixture, and the page a design change is reviewed on.

const at = (iso: string) => iso as Instant;
const NOW = epochMs(at("2026-11-03T09:00:00Z"));

const GEPARKT: GeparkteBuchung = {
  key: "rechnung:9900357000004:51238696012:R-2026-0815",
  art: "RECHNUNG",
  malo_id: "51238696012",
  lf_mp_id: "9900357000004",
  betrag_ct: 900,
  buchungsdatum: "2026-10-31" as BusinessDate,
  beleg: "R-2026-0815",
  code: 301,
  meldung: "period sealed through 2026-10-31",
  hinweis: "Die Periode ist festgeschrieben. Auf ein Datum im ersten offenen Monat neu buchen.",
  vorschlag_datum: "2026-11-01" as BusinessDate,
  seit: at("2026-11-03T07:00:00Z"),
};

const BILANZ: Bilanz = {
  tage: 30,
  laeufe: 17,
  offen: 1,
  abgeschlossen: { succeeded: 16 },
  hilfreich: { ja: 6, nein: 1 },
  vorschlag: { sent: 14, edited: 2, dismissed: 1 },
};

/** A proposal agentd prepared, as `/vorschlaege` answers it. */
const wartend = (run: string, v: Vorschlag | null, status = "succeeded"): Answer => ({
  run,
  status,
  disclosure: "AI-generated.",
  saetze: [],
  vorschlag: v,
  schritte: [],
  hilfreich: null,
});

const NEU_BUCHEN = wartend("r-g1", {
  aktion: "neu_buchen",
  buchungsdatum: "2026-11-01",
  grund: null,
  antwortcode: null,
  befund: null,
  begruendung: "Der Oktober ist festgeschrieben; der erste offene Tag ist der 01.11.2026.",
  quelle: "accountingd/get_parked_booking",
  belegt: true,
});

const VERWERFEN = wartend("r-g2", {
  aktion: "verwerfen",
  buchungsdatum: null,
  grund: "R-2026-0815 ist am 14.10.2026 bereits gebucht.",
  antwortcode: null,
  befund: null,
  begruendung: "Das Konto führt R-2026-0815 mit 9,00 € am 14.10.2026.",
  quelle: "accountingd/list_ledger",
  belegt: false,
});

const WARUM_ANTWORT = {
  ...wartend("r-w", null),
  saetze: [
    {
      text: "Das Hauptbuch hat die Rechnung R-2026-0815 für den 31.10.2026 abgelehnt.",
      quelle: "accountingd/get_parked_booking",
      belegt: true,
    },
    {
      text: "Neu gebucht werden kann sie ab dem 01.11.2026.",
      quelle: "accountingd/get_parked_booking",
      belegt: false,
    },
    { text: "Entscheiden Sie: neu buchen oder verwerfen.", quelle: null, belegt: false },
  ],
  schritte: [{ quelle: "accountingd/get_parked_booking", fertig: true }],
};

/**
 * A trail as agentd's stream sends it: three reads, the second one hidden
 * from the person (a forward case — agentd sends none hidden yet).
 */
const SPUR: Schritt[] = [
  {
    seq: 3,
    art: "lesen",
    was: "gelesen",
    objekt: "Buchung R-2026-0815",
    quelle: { dienst: "accountingd", werkzeug: "get_parked_booking", ref: "R-2026-0815" },
    verborgen: false,
    um: at("2026-11-03T09:00:00.300Z"),
  },
  {
    seq: 5,
    art: "lesen",
    was: "gelesen",
    objekt: "Kunde k-1",
    quelle: null,
    verborgen: true,
    um: at("2026-11-03T09:00:00.900Z"),
  },
  {
    seq: 7,
    art: "lesen",
    was: "aufgelistet",
    objekt: "MaLo 51238696012",
    quelle: { dienst: "accountingd", werkzeug: "list_ledger", ref: "51238696012" },
    verborgen: false,
    um: at("2026-11-03T09:00:02.400Z"),
  },
];

const LAUF: Lauf = {
  run: "r-p",
  frage: "Was ist hier offen?",
  anker: '{"malo":"51238696012"}',
  rolle: "netz",
  zurueck: "/prozess?key=51238696012",
  pille: true,
  nr: 1,
  schritte: SPUR.slice(0, 1),
  antwort: null,
  abgebrochen: null,
  unlesbar: false,
  stoppt: false,
  stoppFehler: null,
  gelesen: false,
  zuschauer: 0,
};

function Specimen({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h2 className="m-0 text-caption font-semibold uppercase tracking-wide text-text-2">
        {title}
      </h2>
      <div className="flex flex-wrap items-center gap-3">{children}</div>
    </section>
  );
}

/** The layout at the width the gallery gives it: one pane, or two. */
function ListeDetailProbe() {
  const [gewaehlt, setGewaehlt] = useState<string | null>("b");
  const zeilen = [
    { id: "a", malo: "51238696012", zustand: "läuft" },
    { id: "b", malo: "41203768152", zustand: "ohne Bestätigung" },
  ];
  const z = zeilen.find((r) => r.id === gewaehlt);
  return (
    <ListDetail
      hoehe="h-96"
      label="Beispiel"
      kopf={<h3 className="m-0 text-title font-semibold">Fristen überschritten</h3>}
      selected={gewaehlt}
      onSelect={setGewaehlt}
      liste={zeilen.map((r) => (
        <ListRow key={r.id} id={r.id} textValue={r.malo} title={r.malo} subtitle={r.zustand} />
      ))}
      detail={
        z ? (
          <div className="px-6 py-6">
            <h2 className="m-0 font-mono text-title font-semibold">{z.malo}</h2>
          </div>
        ) : null
      }
      angaben={
        z ? (
          <dl className="m-0">
            <Angabe name="MaLo">{z.malo}</Angabe>
            <Angabe name="Zustand">{z.zustand}</Angabe>
          </dl>
        ) : null
      }
    />
  );
}

function Specimens() {
  return (
    <div className="flex flex-col gap-8">
      <Specimen title="Schaltflächen">
        <Button variant="primary">Freigeben</Button>
        <Button>Zurückstellen</Button>
        <Button variant="ghost">Abbrechen</Button>
        <Button variant="primary" isDisabled>
          Gesperrt
        </Button>
      </Specimen>

      <Specimen title="Frist-Uhr">
        <FristUhr due={at("2026-11-03T07:00:00Z")} now={NOW} />
        <FristUhr due={at("2026-11-03T10:15:00Z")} now={NOW} />
        <FristUhr due={at("2026-11-03T16:00:00Z")} now={NOW} />
        <FristUhr due={at("2026-11-06T11:00:00Z")} now={NOW} />
      </Specimen>

      <Specimen title="Quelle">
        <span className="text-callout">
          Antwort bis 11:00 Uhr am 1. WT{" "}
          <SourceChip
            quelle={{
              dokument: "GPKE Teil 2 (BK6-24-174 Anlage 1b)",
              kapitel: "3.1",
              seite: "41",
              regel:
                "Antwort auf die Anmeldung: spätester ÜT ist der 1. WT nach Eingang, 11:00 Uhr.",
            }}
          />
        </span>
      </Specimen>

      <Specimen title="Agent">
        <div className="w-full">
          <AiNote
            text={
              "AI-generated. Die Anmeldung wurde abgelehnt; die Antwort des Netzbetreibers nennt den Grund. Vorschlag: die Anmeldung mit korrigiertem Lieferbeginn erneut senden."
            }
            footer={
              <div className="flex items-center gap-2 text-caption text-text-2">
                gelesen: <code className="font-mono">get_process</code>
                <span className="flex-1" />
                <Button variant="ghost">Erneut</Button>
                <Button variant="ghost">Ausblenden</Button>
              </div>
            }
          />
        </div>
      </Specimen>

      <Specimen title="Suchfeld">
        <div className="w-full">
          <SearchField aria-label="Suche" placeholder="MaLo-ID, MeLo-ID oder Vorgangsnummer" />
        </div>
        <div className="w-64">
          <SearchField
            aria-label="Suche"
            placeholder="Suchen"
            size="compact"
            defaultValue="51238696012"
          />
        </div>
      </Specimen>

      <Specimen title="Tasten">
        <Kbd>⌘K</Kbd>
        <Kbd>J</Kbd>
        <Kbd>K</Kbd>
        <Kbd>Space</Kbd>
        <Kbd>⌃1</Kbd>
        <Kbd>?</Kbd>
      </Specimen>

      <Specimen title="Rollen">
        {(Object.keys(ROLLEN) as Rolle[]).map((r) => (
          <span
            key={r}
            data-rolle={r}
            className="inline-flex h-7 items-center gap-2 rounded-full bg-[color-mix(in_srgb,var(--accent)_12%,transparent)] px-3 text-body font-semibold text-accent"
          >
            <span className="size-2 rounded-full bg-accent" />
            {ROLLEN[r].label}
          </span>
        ))}
      </Specimen>

      <Specimen title="Kachel">
        <div className="grid w-full grid-cols-4 gap-3">
          <Tile
            label="Anmeldungen offen"
            source="Kundenverwaltung"
            href="/heute"
            state={{ kind: "count", n: 1284 }}
          />
          <Tile
            label="Fristen überschritten"
            source="Prozessüberwachung"
            href="/fristen"
            state={{ kind: "count", n: 500, atLeast: true, breach: true }}
          />
          <Tile
            label="Dead Letters"
            source="Marktkommunikation"
            href="/prozess"
            state={{ kind: "loading" }}
          />
          <Tile
            label="Dead Letters"
            source="Marktkommunikation"
            href="/prozess"
            state={{ kind: "failed", service: "Marktkommunikation" }}
          />
          <Tile
            label="Assistent"
            source="Assistent"
            href="/assistent"
            ziel="Beipackzettel öffnen"
            state={{
              kind: "wert",
              wert: "86 %",
              zeilen: ["hilfreich · 7 Bewertungen in 30 Tagen", "14 von 17 Vorschlägen übernommen"],
            }}
          />
          <Tile
            label="Assistent"
            source="Assistent"
            href="/assistent"
            ziel="Beipackzettel öffnen"
            state={{
              kind: "wert",
              zeilen: ["2 Bewertungen in 30 Tagen", "Noch kein Vorschlag entschieden"],
            }}
          />
        </div>
      </Specimen>

      <Specimen title="Warum?">
        <div className="grid w-full gap-3">
          <WarumAntwort stand={{ kind: "liest", schritte: [] }} />
          <WarumAntwort
            stand={{
              kind: "liest",
              schritte: [
                { quelle: "accountingd/get_parked_booking", fertig: true },
                { quelle: "accountingd/list_ledger", fertig: false },
              ],
            }}
          />
          <WarumAntwort stand={{ kind: "antwort", a: WARUM_ANTWORT }} />
          <WarumAntwort
            stand={{ kind: "antwort", a: { ...WARUM_ANTWORT, status: "failed" } }}
            onNochmal={() => undefined}
          />
          <WarumAntwort
            stand={{
              kind: "fehler",
              text: "Der Assistent ist nicht erreichbar. Alles andere im Desk funktioniert weiter.",
            }}
          />
        </div>
      </Specimen>
      <Specimen title="Spur des Assistenten">
        <div className="grid w-full gap-3">
          <WarumAntwort
            stand={{ kind: "liest", schritte: [], spur: SPUR.slice(0, 2) }}
            stopp={{ run: "r-g", lauf: undefined }}
          />
          <WarumAntwort
            stand={{ kind: "liest", schritte: [], spur: SPUR.slice(0, 1) }}
            stopp={{ run: "r-g", lauf: { ...LAUF, stoppt: true } }}
          />
          <WarumAntwort stand={{ kind: "antwort", a: WARUM_ANTWORT, spur: SPUR }} />
          <WarumAntwort
            stand={{
              kind: "abgebrochen",
              spur: [...SPUR.slice(0, 1), { ...(SPUR[2] as Schritt), art: "fehlgeschlagen" }],
            }}
          />
          <div className="rounded-panel border border-ai-border bg-ai-fill p-4">
            <SpurAnsicht schritte={SPUR} stand="beantwortet" anfangsOffen />
          </div>
        </div>
      </Specimen>

      <Specimen title="Laufende Frage in der Leiste">
        <LaufPill lauf={LAUF} onPress={() => undefined} />
        <LaufPill
          lauf={{ ...LAUF, schritte: SPUR, antwort: WARUM_ANTWORT }}
          onPress={() => undefined}
        />
        <LaufPill
          lauf={{ ...LAUF, abgebrochen: { durch: "sb", um: at("2026-11-03T09:00:01Z") } }}
          onPress={() => undefined}
        />
      </Specimen>

      <Specimen title="Aktionen in ⌘K">
        <AktionenVorschau bilanz={BILANZ} />
      </Specimen>

      <Specimen title="Geparkte Buchung">
        <div className="grid w-full gap-3">
          <GeparktEntscheiden
            b={GEPARKT}
            onDone={() => undefined}
            vorschlag={NEU_BUCHEN}
            bilanz={BILANZ}
          />
          <GeparktEntscheiden
            b={GEPARKT}
            onDone={() => undefined}
            vorschlag={VERWERFEN}
            bilanz={{ ...BILANZ, vorschlag: { sent: 2 } }}
          />
          <GeparktEntscheiden
            b={GEPARKT}
            onDone={() => undefined}
            vorschlag={wartend("r-g3", null, "running")}
            bilanz={BILANZ}
          />
          <GeparktEntscheiden b={GEPARKT} onDone={() => undefined} />
          <GeparktEntscheiden
            b={GEPARKT}
            onDone={() => undefined}
            anfang={{ aktion: "verwerfen", grund: "Doppelt erfasst", gehalten: true }}
          />
          <GeparktEntscheiden
            b={GEPARKT}
            onDone={() => undefined}
            anfang={{
              aktion: "buchen",
              fehler: new Abgelehnt("period sealed through 2026-11-30"),
            }}
          />
        </div>
      </Specimen>

      <Specimen title="Zeilen">
        <Panel className="w-full overflow-hidden">
          <GridList
            aria-label="Zeilen"
            selectionMode="single"
            selectionBehavior="replace"
            defaultSelectedKeys={["gewaehlt"]}
          >
            <ListRow
              id="ruhe"
              textValue="51238696012"
              title="51238696012"
              subtitle="PID 55001 · läuft"
            />
            <ListRow
              id="gewaehlt"
              textValue="41203768152"
              title="41203768152"
              subtitle="PID 55004 · ohne Bestätigung"
              trailing={<FristUhr due={at("2026-11-03T08:00:00Z")} now={NOW} />}
            />
            <ListRow
              id="fuehrend"
              textValue="Erika Mustermann"
              leading={
                <span className="flex size-9 items-center justify-center rounded-full bg-fill-strong text-body font-semibold">
                  EM
                </span>
              }
              title="Erika Mustermann"
              subtitle="K-10001 · 2 Lieferstellen"
              trailing={<Icon name="chevron" size={16} className="text-text-2" />}
            />
          </GridList>
        </Panel>
      </Specimen>

      <Specimen title="Tabelle">
        <Panel className="w-full overflow-hidden">
          <Table
            label="Dead Letters"
            spalten={[
              {
                id: "z",
                titel: "Zeitpunkt",
                zelle: (r: string[]) => r[0],
                className: "w-36 text-text-2",
              },
              { id: "w", titel: "Was geschah", rowHeader: true, zelle: (r: string[]) => r[1] },
              {
                id: "k",
                titel: "Kategorie",
                zelle: (r: string[]) => r[2],
                className: "font-mono text-text-2",
              },
            ]}
            zeilen={[
              ["07.11.2026, 10:15", "PID 99999 is not registered", "unknown_pid"],
              ["06.11.2026, 18:02", "delivery failed after 12 attempts", "delivery"],
            ]}
            zeilenId={(_, i) => `${i}`}
          />
        </Panel>
        <Panel className="w-full overflow-hidden">
          <Table
            label="Leere Tabelle"
            spalten={[{ id: "w", titel: "Was geschah", rowHeader: true, zelle: (r: string) => r }]}
            zeilen={[]}
            zeilenId={(r) => r}
            leer={<p className="m-0 px-5 py-6 text-body text-text-2">Keine Zeilen.</p>}
          />
        </Panel>
      </Specimen>

      <Specimen title="Liste, Detail, Angaben">
        <Panel className="w-full overflow-hidden">
          <ListeDetailProbe />
        </Panel>
      </Specimen>

      <Specimen title="Zustände">
        <div className="grid w-full gap-3">
          <Notice tone="refusal" title="Kein Prozess für „51238696012“">
            process_not_found: No process for business key '51238696012'.
          </Notice>
          <Notice tone="info" title="Der Assistent liest nur.">
            Keine Fähigkeit kann eine Marktnachricht senden.
          </Notice>
          <Notice tone="failure" title="Marktkommunikation ist nicht erreichbar">
            <code className="font-mono">just desk</code> startet makod mit dem Desk.
          </Notice>
          <Panel>
            <EmptyState title="Alles erledigt.">
              Die nächste Frist läuft morgen um 11:00 Uhr ab.
            </EmptyState>
          </Panel>
        </div>
      </Specimen>

      <Specimen title="Einrichtung">
        <div className="flex w-full flex-col gap-3">
          <div className="flex flex-wrap gap-4">
            {["gruen", "gelb", "rot", "unbekannt"].map((a) => (
              <AmpelZeichen key={a} ampel={a} />
            ))}
          </div>
          <ul className="m-0 flex list-none flex-wrap gap-2 p-0">
            <DienstChip zeile={{ dienst: "makod", url: "", status: "bereit", version: "0.31.0" }} />
            <DienstChip
              zeile={{ dienst: "processd", url: "", status: "blockiert", version: "0.31.0" }}
            />
            <DienstChip zeile={{ dienst: "edmd", url: "", status: "unerreichbar" }} />
            <DienstChip zeile={{ dienst: "billingd", url: "", status: "unlesbar" }} />
          </ul>
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            <BefundZeile
              befund={{
                dienst: "makod",
                schwere: "blockiert",
                bereich: "identitaeten",
                text: '"9900001000001" ist eine BDEW-Codenummer (Strom), aber die Rollen ["GNB"] gehören zur anderen Sparte',
                pfad: "party[0].mp_id",
                quelle: "BDEW-AWH Identifikatoren V1.2 Kap. 2.2",
              }}
            />
            <BefundZeile
              befund={{
                dienst: "makod",
                schwere: "warnung",
                bereich: "konfiguration",
                text: "as4.unbekannte_absender steht auf bdew_ou_zulassen",
                pfad: "policy.as4.unbekannte_absender",
                referenz: "@/docs/services/makod.md#policy-as4-unbekannte-absender",
              }}
            />
            <BefundZeile
              befund={{
                dienst: "edmd",
                schwere: "warnung",
                bereich: "erreichbarkeit",
                text: "nicht erreichbar: connection refused",
                pfad: "",
              }}
            />
          </ul>
        </div>
      </Specimen>

      <Specimen title="Vorschlag">
        <div className="w-full max-w-xl">
          <DateiAnsicht
            datei={{
              dienst: "processd",
              datei: "processd.toml",
              abgeglichen: false,
              aenderungen: 2,
              luecken: 1,
              toml: "",
              diff: "",
              zeilen: [
                { art: "abschnitt", text: "[identity]" },
                {
                  art: "neu",
                  text: 'own_mp_id = "9900357000004"',
                  quelle: "processd [identity] own_mp_id",
                  frage: "primaer",
                },
                {
                  art: "gleich",
                  text: "verbundene_lieferanten = []",
                  quelle: "BDEW-AWH Identifikatoren V1.2, Kap. 2.1",
                  frage: "verbundene_lieferanten",
                },
                {
                  art: "weg",
                  text: 'own_mp_id = "9900001000001"',
                  quelle: "processd [identity] own_mp_id",
                  frage: "primaer",
                },
                {
                  art: "fehlt",
                  text: '# FEHLT: [[lf.bilanzkreise]] sparte = "gas"',
                  quelle: "processd [[lf.bilanzkreise]]",
                  frage: "bilanzkreise.gas",
                },
              ],
            }}
          />
        </div>
      </Specimen>

      <Specimen title="Symbole">
        {(
          [
            "search",
            "clock",
            "sparkle",
            "doc",
            "tray",
            "flow",
            "grid",
            "copy",
            "check",
            "warn",
            "lock",
            "stop",
            "menu",
            "blockiert",
            "unbekannt",
            "erneuern",
          ] as const
        ).map((n) => (
          <Icon key={n} name={n} size={20} />
        ))}
      </Specimen>
    </div>
  );
}

/** /dev/gallery — light and dark, side by side. */
export function Gallery() {
  return (
    <div className="grid min-h-full grid-cols-2">
      {(["light", "dark"] as const).map((theme) => (
        <div key={theme} data-theme={theme} className="bg-bg px-8 py-10 text-text">
          <p className="m-0 mb-6 text-caption font-semibold uppercase tracking-wide text-text-2">
            {theme === "light" ? "Hell" : "Dunkel"}
          </p>
          <Specimens />
        </div>
      ))}
    </div>
  );
}
