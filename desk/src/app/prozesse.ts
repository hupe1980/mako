// What an operator calls each process, by the workflow makod runs it under.
// Presentation only: a title decides nothing, and a workflow not listed shows
// its own name. The workflow's name is never parsed for meaning.

const TITEL: Record<string, string> = {
  "gpke-supplier-change": "Lieferantenwechsel · Strom",
  "gpke-lf-anmeldung": "Anmeldung beim Netzbetreiber · Strom",
  "gpke-lf-abmeldung": "Abmeldung beim Netzbetreiber · Strom",
  "gpke-beendigung-zuordnung": "Beendigung der Zuordnung · Strom",
  "gpke-ankuendigung-zuordnung-lf": "Ankündigung der Zuordnung · Strom",
  "gpke-zuordnungsmeldung": "Zuordnungsmeldung · Strom",
  "gpke-kuendigung": "Kündigung · Strom",
  "gpke-stornierung": "Stornierung · Strom",
  "gpke-eog": "Ersatz- und Grundversorgung · Strom",
  "gpke-neuanlage": "Neuanlage · Strom",
  "gpke-stammdatenaenderung": "Stammdatenänderung · Strom",
  "gpke-anfrage-bestellung": "Anfrage und Bestellung · Strom",
  "gpke-sperrung": "Sperrung · Strom",
  "gpke-sperrung-lf": "Sperrauftrag des Lieferanten · Strom",
  "gpke-konfiguration": "Konfiguration · Strom",
  "gpke-konfiguration-aenderung": "Änderung der Konfiguration · Strom",
  "gpke-datenabruf": "Datenabruf · Strom",
  "gpke-geschaeftsdatenanfrage": "Geschäftsdatenanfrage · Strom",
  "gpke-messwerte": "Messwerte · Strom",
  "gpke-abrechnung": "Abrechnung · Strom",
  "gpke-abrechnungsdaten": "Abrechnungsdaten · Strom",
  "gpke-allokationsliste": "Allokationsliste",
  "gpke-partin": "Kommunikationsdaten · Strom",
  "gpke-utilts": "UTILTS · Strom",
  "geli-gas-supplier-change": "Lieferantenwechsel · Gas",
  "geli-gas-lf-anmeldung": "Anmeldung beim Netzbetreiber · Gas",
  "geli-gas-zuordnungsmeldung": "Zuordnungsmeldung · Gas",
  "geli-gas-stornierung": "Stornierung · Gas",
  "geli-gas-stornierung-lf": "Stornierung · Gas",
  "geli-gas-stammdatenaenderung": "Stammdatenänderung · Gas",
  "geli-gas-sperrung-nb": "Sperrung · Gas",
  "geli-gas-sperrung-lf": "Sperrauftrag des Lieferanten · Gas",
  "geli-gas-sperrprozesse-invoic": "Rechnung zur Sperrung · Gas",
  "geli-gas-datenabruf": "Datenabruf · Gas",
  "geli-gas-mscons": "Messwerte · Gas",
  "geli-gas-partin": "Kommunikationsdaten · Gas",
  "wim-device-change": "Messstellenbetreiberwechsel",
  "wim-ersteinbau": "Ersteinbau",
  "wim-geraeteubernahme": "Geräteübernahme",
  "wim-insrpt": "Störungsmeldung",
  "wim-invoic": "Rechnung des Messstellenbetreibers",
  "wim-preisanfrage": "Preisanfrage",
  "wim-preisliste": "Preisliste",
  "wim-rechnungsabwicklung": "Rechnungsabwicklung",
  "wim-steuerungsauftrag": "Steuerungsauftrag",
  "wim-technik-aenderung": "Änderung der Technik",
  "wim-weiterverpflichtung": "Weiterverpflichtung",
  "wim-wertebestellung": "Wertebestellung",
  "esa-wertebestellung": "Wertebestellung des Energieserviceanbieters",
  "mabis-billing": "Bilanzkreisabrechnung",
  "mabis-clearingliste": "Clearingliste",
  "mabis-listenabgleich": "Listenabgleich",
  "mabis-profile": "Lastprofile",
  "mabis-anforderung": "Anforderung von Bilanzierungsdaten",
  "mabis-zp-lifecycle": "Zählpunkt in der Bilanzierung",
  "gabi-gas-allocation": "Allokation · Gas",
  "gabi-gas-nomination": "Nominierung · Gas",
  "gabi-gas-mmma": "Mehr-/Mindermengen-Allokation · Gas",
  "gabi-gas-mehr-mindermengen": "Mehr-/Mindermengen · Gas",
  "gabi-gas-invoic": "Rechnung · GaBi Gas",
  "redispatch-aktivierung": "Redispatch-Aktivierung",
  "redispatch-kaskade": "Redispatch-Kaskade",
  "redispatch-kostenblatt": "Redispatch-Kostenblatt",
  "redispatch-netzengpass": "Netzengpass",
  "redispatch-planungsdaten": "Planungsdaten",
  "redispatch-stammdaten": "Redispatch-Stammdaten",
  "redispatch-statusanfrage": "Statusanfrage",
  "redispatch-verfuegbarkeit": "Verfügbarkeit · Redispatch",
};

/** The process's title for an operator, or its workflow name where none is listed. */
export function prozessTitel(workflow: string): string {
  return TITEL[workflow] ?? workflow;
}

const ZUSTAND: Record<string, string> = {
  initiated: "begonnen",
  running: "läuft",
  antwortfrist_abgelaufen: "ohne Bestätigung",
  completed: "abgeschlossen",
  rejected: "abgelehnt",
  failed: "fehlgeschlagen",
};

/** A process state as processd names it, in the operator's words. */
export function zustand(state: string): string {
  return ZUSTAND[state] ?? state;
}

const FRIST_ART: Record<string, string> = {
  aperak: "Eingangsbestätigung (APERAK)",
  contrl: "Syntaxquittung (CONTRL)",
  prozess: "Frist des Prozesses",
};

/** What a pending deadline is for, by the kind makod states; the kind itself where it is unknown. */
export function fristArt(art: string): string {
  return FRIST_ART[art] ?? art;
}
