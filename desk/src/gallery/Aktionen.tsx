import { type Aktion, aktionen, type Gefunden } from "../app/aktionen";
import { AktionForm, AktionPanel } from "../app/CommandPalette";
import type { Answer, Bilanz } from "../data/agent";
import type { Freigabe } from "../data/gen/processd";
import { Button } from "../design/Button";
import { HaltRing } from "../design/HaltRing";
import type { Instant } from "../format/brands";

// ⌘K's action panel and an action's parameters, each state as a still: the
// palette's own dialog is modal, so the gallery shows its two panels bare.

const FREIGABE: Freigabe = {
  id: "q-1",
  process_id: "p-1",
  pid: 55001,
  malo_id: "51238696012",
  marktrolle: "NB",
  grund: "Anmeldung eines verbundenen Lieferanten",
  art: "eskaliert",
  antwort: null,
  codes_freigeben: { ebd: "E_0623", codes: [{ code: "E15", bedeutung: "Zustimmung" }] },
  codes_ablehnen: {
    ebd: "E_0622",
    codes: [
      { code: "A01", bedeutung: "Marktlokation nicht gefunden" },
      { code: "A02", bedeutung: "nimmt nicht an der Marktkommunikation teil" },
    ],
  },
  fakten_freigeben: [],
  fakten_ablehnen: [],
  kann_freigeben: true,
  kann_ablehnen: true,
  status: "Pending",
  expires_at: "2026-11-04T10:00:00Z" as Instant,
  created_at: "2026-11-03T08:00:00Z" as Instant,
  decided_by: null,
  decided_at: null,
};

const G: Gefunden = {
  titel: "51238696012",
  kennung: true,
  grund: FREIGABE.grund,
  frist: FREIGABE.expires_at,
  ding: { art: "freigabe", f: FREIGABE },
};

const VORSCHLAG: Answer = {
  run: "r-a",
  status: "succeeded",
  disclosure: "AI-generated.",
  saetze: [],
  vorschlag: {
    aktion: "ablehnen",
    antwortcode: "A02",
    begruendung: "Die MaLo nimmt nicht an der Marktkommunikation teil.",
    quelle: "processd/get_queue_entry",
    belegt: true,
  },
  schritte: [],
  hilfreich: null,
};

const ABLEHNEN = aktionen(G.ding).find((a) => a.id === "ablehnen") as Aktion;
const nichts = () => undefined;

function Rahmen({ children }: { children: React.ReactNode }) {
  return (
    <div className="squircle w-[min(600px,100%)] overflow-hidden rounded-panel bg-surface-raised shadow-popover">
      {children}
    </div>
  );
}

export function AktionenVorschau({ bilanz }: { bilanz: Bilanz }) {
  return (
    <div className="grid w-full gap-3">
      <Rahmen>
        <AktionPanel
          g={G}
          wartend={VORSCHLAG}
          onWaehlen={nichts}
          onZurueck={nichts}
          vorschau={{}}
        />
      </Rahmen>
      <Rahmen>
        <AktionPanel
          g={G}
          wartend={undefined}
          onWaehlen={nichts}
          onZurueck={nichts}
          vorschau={{ verweigert: "Reject denied for this principal" }}
        />
      </Rahmen>
      <Rahmen>
        <AktionForm
          g={G}
          a={ABLEHNEN}
          wartend={VORSCHLAG}
          onZurueck={nichts}
          onGehalten={nichts}
          vorschau={{ bilanz }}
        />
      </Rahmen>
      <Rahmen>
        <AktionForm
          g={G}
          a={ABLEHNEN}
          wartend={undefined}
          onZurueck={nichts}
          onGehalten={nichts}
          vorschau={{}}
        />
      </Rahmen>
      <Rahmen>
        <AktionForm
          g={G}
          a={ABLEHNEN}
          wartend={undefined}
          onZurueck={nichts}
          onGehalten={nichts}
          vorschau={{ verweigert: "Reject denied for this principal" }}
        />
      </Rahmen>
      <div className="flex items-center gap-3">
        <HaltRing ms={5_000} label="Gleich gesendet: Freigabe abgelehnt — ⌘Z nimmt es zurück" />
        <Button variant="destructive">Ablehnen</Button>
        <Button variant="destructive" isDisabled>
          Ablehnen
        </Button>
      </div>
    </div>
  );
}
