import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button as AriaButton } from "react-aria-components";
import { belegSchluessel, entmarkieren, markieren, useBelegZiel } from "../app/belege";
import { quelle as quelleName, schrittZeile } from "../app/dienste";
import { STOPP } from "../app/keys";
import { type Lauf, stoppen, useLauf, useZuschauen } from "../app/laeufe";
import { useRolle } from "../app/rollen";
import { type Answer, antwort, type Schritt } from "../data/agent";
import { Icon } from "../design/Icon";
import { SourceChip } from "../design/SourceChip";
import { formatCount } from "../format/count";

/** Where a run stands for its view: what its stream said, or its answer read. */
export interface LaufStand {
  lauf: Lauf | undefined;
  /** The conclusion — streamed, or read where no stream could be. */
  antwort: Answer | undefined;
  /** Still reading: Stop is offered and Esc stops it. */
  live: boolean;
  /** Steps are streamed — the trail shows them, the receipt sums them up. */
  gestreamt: boolean;
  fehler: Error | null;
  /** Without a stream: the reads the answer read names so far. */
  gelesen: Answer["schritte"];
}

/**
 * A run as its view shows it. The stream is the source; where agentd's stream
 * cannot be read the view reads the answer as before, every second until it
 * concludes. Mounting this marks the view as showing the run, so its pill
 * leaves the top bar and its conclusion counts as read.
 */
export function useLaufStand(run: string | undefined): LaufStand {
  const { rolle } = useRolle();
  const lauf = useLauf(run);
  useZuschauen(run);
  const lesen = Boolean(run) && (!lauf || lauf.unlesbar);
  const answer = useQuery({
    queryKey: [rolle, "agentd", "antwort", run],
    queryFn: () => antwort(run ?? ""),
    enabled: lesen,
    refetchInterval: (q) => (q.state.data?.status === "running" ? 1000 : false),
  });
  const gelesen = lesen ? answer.data : undefined;
  const fertig = lauf?.antwort ?? (gelesen && gelesen.status !== "running" ? gelesen : undefined);
  const live = Boolean(run) && !fertig && !lauf?.abgebrochen && !(lesen && answer.isError);
  return {
    lauf,
    antwort: fertig,
    live,
    gestreamt: Boolean(lauf && (lauf.schritte.length > 0 || !lauf.unlesbar)),
    fehler: lesen ? answer.error : null,
    gelesen: gelesen?.schritte ?? [],
  };
}

/** Esc stops `run` while it is live — unless a dialog or popover owns Esc. */
export function useStoppTaste(run: string | undefined, live: boolean): void {
  useEffect(() => {
    if (!run || !live) return;
    const onKey = (e: KeyboardEvent) => {
      if (!STOPP.match?.(e)) return;
      e.preventDefault();
      stoppen(run);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [run, live]);
}

/** Stop, in the place Send stood. */
export function StoppKnopf({ lauf, run }: { lauf: Lauf | undefined; run: string }) {
  const stoppt = Boolean(lauf?.stoppt);
  return (
    <AriaButton
      onPress={() => stoppen(run)}
      isDisabled={stoppt}
      aria-keyshortcuts="Escape"
      className="morph focus-ring inline-flex h-9 items-center gap-2 rounded-control border border-ai bg-surface px-3 text-body font-semibold text-ai data-[hovered]:bg-ai-fill data-[disabled]:opacity-60"
    >
      <Icon name="stop" size={12} />
      {stoppt ? "Wird gestoppt …" : "Stoppen"}
      {!stoppt && (
        <kbd aria-hidden className="font-sans text-caption font-medium">
          esc
        </kbd>
      )}
    </AriaButton>
  );
}

/** The SourceChip of one read: the service as an operator knows it, and what was read. */
function chipQuelle(s: Schritt) {
  const q = s.quelle;
  if (!q) return null;
  return {
    dokument: quelleName(`${q.dienst}/${q.werkzeug}`),
    regel: schrittZeile(s),
  };
}

function SchrittZeile({ s, nr, abgebrochen }: { s: Schritt; nr: number; abgebrochen: boolean }) {
  const ziel = useBelegZiel(s.quelle ? belegSchluessel(s.quelle) : null);
  const chip = chipQuelle(s);
  // A call that did not answer — one the stop refused, say — stays, muted.
  const fehlgeschlagen = s.art === "fehlgeschlagen";
  return (
    <li
      {...ziel}
      data-schritt={s.seq}
      tabIndex={-1}
      className="einblenden flex flex-wrap items-center gap-x-2 gap-y-1 rounded-[6px] text-callout text-text-2 outline-none"
    >
      <span className="inline-flex size-5 shrink-0 items-center justify-center rounded-full bg-surface text-caption font-semibold text-ai tabular-nums">
        {nr}
      </span>
      <span className={fehlgeschlagen || abgebrochen ? "text-text-2" : "text-text"}>
        {schrittZeile(s)}
      </span>
      {chip ? (
        <SourceChip quelle={chip} />
      ) : (
        <span
          title="Diese Quelle dürfen Sie nicht lesen; der Schritt bleibt sichtbar, ihr Inhalt nicht."
          className="inline-flex h-5 items-center gap-1 rounded-full border border-separator px-2 text-caption font-medium text-text-2"
        >
          <Icon name="lock" size={11} />
          Quelle für Sie verborgen
        </span>
      )}
      {fehlgeschlagen && (
        <span className="inline-flex h-5 items-center rounded-full bg-fill px-2 text-caption font-medium text-text-2">
          nicht beantwortet
        </span>
      )}
      {abgebrochen && (
        <span className="inline-flex h-5 items-center rounded-full bg-fill px-2 text-caption font-semibold text-text-2">
          abgebrochen
        </span>
      )}
    </li>
  );
}

/**
 * The receipt: how many reads with a source the person may see, counted off
 * the stream's events. No duration — the journal stamps no time on a record,
 * so agentd's `um` is when it served the step, not when the read happened.
 */
function quittung(schritte: Schritt[]): string {
  const n = schritte.filter((s) => s.quelle).length;
  return `${formatCount(n)} ${n === 1 ? "Quelle" : "Quellen"} gelesen`;
}

/**
 * The trail: one line per read, in journal order, each with its source — a
 * hairline above it while the run is live. Once the answer stands it folds
 * into one receipt line that opens it again; a stopped run keeps its steps
 * open, each marked „abgebrochen".
 */
export function SpurAnsicht({
  schritte,
  stand,
  oeffnen,
  anfangsOffen = false,
}: {
  schritte: Schritt[];
  stand: "live" | "beantwortet" | "abgebrochen";
  /** The receipt starts opened — the gallery's state of a reopened trail. */
  anfangsOffen?: boolean;
  /** A citation asks for this step: the trail opens and focuses it. */
  oeffnen?: { seq: number; mal: number } | null | undefined;
}) {
  const [offen, setOffen] = useState(anfangsOffen);
  const liste = useRef<HTMLOListElement>(null);
  useEffect(() => {
    if (!oeffnen) return;
    setOffen(true);
  }, [oeffnen]);
  useEffect(() => {
    if (!oeffnen || !offen) return;
    const zeile = liste.current?.querySelector<HTMLElement>(`[data-schritt="${oeffnen.seq}"]`);
    const ziel = zeile?.querySelector<HTMLElement>("button") ?? zeile;
    ziel?.scrollIntoView?.({ block: "nearest" });
    ziel?.focus();
  }, [oeffnen, offen]);

  const zeigen = stand !== "beantwortet" || offen;
  return (
    <div className="mt-3">
      {stand === "live" && (
        <div role="status" aria-label="Der Assistent liest" className="haarlinie mb-3" />
      )}
      {stand === "beantwortet" && schritte.length > 0 && (
        <AriaButton
          onPress={() => setOffen((o) => !o)}
          aria-expanded={offen}
          className="focus-ring inline-flex h-6 items-center gap-1 rounded-full px-2 text-caption font-medium text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
        >
          {quittung(schritte)}
          <span aria-hidden className={`morph inline-block ${offen ? "rotate-90" : ""}`}>
            ▸
          </span>
        </AriaButton>
      )}
      {stand === "live" && schritte.length === 0 && (
        <p className="m-0 text-callout text-text-2">Der Assistent überlegt, was er lesen muss …</p>
      )}
      {zeigen && schritte.length > 0 && (
        <ol
          ref={liste}
          aria-label="Was der Assistent gelesen hat"
          className={`einblenden m-0 flex list-none flex-col gap-1.5 p-0 ${stand === "beantwortet" ? "mt-2" : ""}`}
        >
          {schritte.map((s, i) => (
            <SchrittZeile key={s.seq} s={s} nr={i + 1} abgebrochen={stand === "abgebrochen"} />
          ))}
        </ol>
      )}
      {stand === "abgebrochen" && (
        <p className="m-0 mt-2 text-callout text-text-2">
          Gestoppt — der Assistent liest nicht weiter. Was er schon gelesen hat, bleibt hier.
        </p>
      )}
    </div>
  );
}

/**
 * A sentence's source numbers: the steps of the trail whose read it cites.
 * Focus or the pointer on one marks every row on screen showing that read;
 * pressing it opens the trail on the step.
 */
export function Belege({
  satz,
  schritte,
  onOeffnen,
}: {
  satz: Answer["saetze"][number];
  schritte: Schritt[];
  onOeffnen: (seq: number) => void;
}) {
  if (!satz.quelle) return null;
  const zitiert = schritte.flatMap((s, i) =>
    s.quelle && `${s.quelle.dienst}/${s.quelle.werkzeug}` === satz.quelle
      ? [{ s, nr: i + 1, key: belegSchluessel(s.quelle) }]
      : [],
  );
  if (zitiert.length === 0) return null;
  return (
    <span className="inline-flex gap-1">
      {zitiert.map(({ s, nr, key }) => (
        <AriaButton
          key={s.seq}
          aria-label={`Beleg ${nr}`}
          onFocus={() => markieren(key)}
          onBlur={() => entmarkieren(key)}
          onHoverStart={() => markieren(key)}
          onHoverEnd={() => entmarkieren(key)}
          onPress={() => onOeffnen(s.seq)}
          className="focus-ring inline-flex size-5 items-center justify-center rounded-full border border-ai-border text-caption font-semibold text-ai tabular-nums data-[hovered]:bg-ai-fill"
        >
          {nr}
        </AriaButton>
      ))}
    </span>
  );
}

/** Where the trail is asked to open, bumped on every press so a second press refocuses. */
export function useOeffnen(): [{ seq: number; mal: number } | null, (seq: number) => void] {
  const [o, setO] = useState<{ seq: number; mal: number } | null>(null);
  return [o, (seq) => setO((alt) => ({ seq, mal: (alt?.mal ?? 0) + 1 }))];
}

/** The trail's state for a run as it stands. */
export function spurStand(stand: LaufStand): "live" | "beantwortet" | "abgebrochen" {
  if (stand.lauf?.abgebrochen) return "abgebrochen";
  return stand.antwort ? "beantwortet" : "live";
}
