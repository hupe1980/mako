import { useRef, useState } from "react";
import type { ProbeBefund, ProbeNachricht, ProbeSegment } from "../data/probe";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { SourceChip } from "../design/SourceChip";
import { type Spalte, Table } from "../design/Table";
import { formatCount } from "../format/count";
import { useT } from "../i18n";

/**
 * Quick Look: a message segment by segment, each named as the profile of the
 * message's own format version names its place, with the findings the
 * service's check placed on it — the failing data element marked where it
 * sits. Every name, position and finding is the service's; the Desk lays them
 * out and decides nothing.
 */
export function QuickLook({ nachricht }: { nachricht: ProbeNachricht }) {
  const t = useT();
  const q = t.quickLook;
  const tabelle = useRef<HTMLDivElement>(null);
  const mitBefund = nachricht.segmente.filter((s) => s.befunde.length > 0);
  const anzahl = nachricht.befunde.length + mitBefund.reduce((n, s) => n + s.befunde.length, 0);
  const quelle =
    nachricht.ahb_version !== null && nachricht.ahb_version !== undefined
      ? `${nachricht.nachrichtentyp} AHB ${nachricht.ahb_version}`
      : null;

  // A grid row takes focus by its key: React Aria renders it as `data-key`.
  const zeige = (position: number) => {
    const row = tabelle.current?.querySelector<HTMLElement>(`[data-key="${position}"]`);
    // Focus scrolls the row into view, at once: no motion to reduce.
    row?.focus();
  };

  const spalten: Spalte<ProbeSegment>[] = [
    {
      id: "pos",
      titel: q.spaltePos,
      className: "w-12 tabular-nums text-text-2",
      zelle: (s) => formatCount(s.position),
    },
    {
      id: "segment",
      titel: q.spalteSegment,
      className: "w-24 whitespace-nowrap",
      zelle: (s) => (
        <span className="flex flex-col">
          <span className="font-mono font-semibold text-text">{s.tag}</span>
          {s.nr && <span className="font-mono text-caption text-text-2">{s.nr}</span>}
        </span>
      ),
    },
    {
      id: "name",
      titel: q.spalteName,
      rowHeader: true,
      className: "w-56",
      zelle: (s) => (
        <span className="flex flex-col">
          <span className={s.name ? "text-text" : "italic text-text-2"}>
            {s.name ?? q.ohneName}
          </span>
          {s.gruppe && <span className="font-mono text-caption text-text-2">{s.gruppe}</span>}
        </span>
      ),
    },
    {
      id: "inhalt",
      titel: q.spalteInhalt,
      zelle: (s) => <Inhalt segment={s} quelle={quelle} />,
    },
  ];

  return (
    <article
      className="flex flex-col gap-4"
      aria-label={q.titel(nachricht.nachrichtentyp, pid(nachricht))}
    >
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="m-0 text-title font-semibold">
          {q.titel(nachricht.nachrichtentyp, pid(nachricht))}
        </h3>
        <span className="font-mono text-body text-text-2">
          {q.empfaenger(nachricht.empfaenger)}
        </span>
        <span className="text-body text-text-2">
          {nachricht.formatversion && nachricht.ahb_version
            ? q.profil(nachricht.formatversion, nachricht.ahb_version)
            : q.profilUnbekannt}
        </span>
      </header>

      {anzahl > 0 && (
        <section
          aria-label={q.befunde(anzahl, formatCount(anzahl))}
          className="flex flex-col gap-1.5"
        >
          <p className="m-0 text-callout font-semibold text-critical">
            {q.befunde(anzahl, formatCount(anzahl))}
          </p>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {mitBefund.map((s) => (
              <li key={s.position}>
                <Button
                  variant="ghost"
                  className="h-auto py-1 text-left"
                  onPress={() => zeige(s.position)}
                >
                  <Icon name="warn" size={14} className="shrink-0 text-critical" />
                  {q.zumSegment(s.tag, formatCount(s.position))}
                  {s.name && <span className="text-text-2">— {s.name}</span>}
                </Button>
              </li>
            ))}
          </ul>
          {nachricht.befunde.length > 0 && (
            <div>
              <p className="m-0 text-caption font-medium text-text-2">{q.lose}</p>
              <Befunde befunde={nachricht.befunde} quelle={quelle} />
            </div>
          )}
        </section>
      )}

      <div ref={tabelle} className="overflow-x-auto rounded-panel border border-separator">
        <Table
          label={q.tabelle}
          spalten={spalten}
          zeilen={nachricht.segmente}
          zeilenId={(s) => `${s.position}`}
        />
      </div>

      <Wire text={nachricht.wire} />
    </article>
  );
}

function pid(n: ProbeNachricht): string {
  return n.pruefidentifikator === null || n.pruefidentifikator === undefined
    ? ""
    : `${n.pruefidentifikator}`;
}

/** The segment's data elements as written, the ones a finding names marked. */
function Inhalt({ segment, quelle }: { segment: ProbeSegment; quelle: string | null }) {
  const t = useT();
  const markiert = new Set(
    segment.befunde.flatMap((b) =>
      b.element === null || b.element === undefined ? [] : [b.element],
    ),
  );
  return (
    <div className="flex flex-col gap-1.5">
      <code className="font-mono text-body break-all text-text">
        <span className="font-semibold">{segment.tag}</span>
        {segment.elemente.map((e, i) => (
          // An element's place is its identity: the list is the segment's own order.
          // biome-ignore lint/suspicious/noArrayIndexKey: positional data elements
          <span key={i}>
            <span className="text-text-2">+</span>
            {markiert.has(i) ? (
              <mark className="rounded-[3px] bg-critical-fill px-0.5 text-critical underline decoration-critical decoration-2 underline-offset-2">
                <span className="sr-only">
                  {`${t.quickLook.fehlerhaft} ${t.quickLook.element(formatCount(i + 1))}: `}
                </span>
                {e === "" ? " " : e}
              </mark>
            ) : (
              e
            )}
          </span>
        ))}
        <span className="text-text-2">'</span>
      </code>
      {segment.befunde.length > 0 && <Befunde befunde={segment.befunde} quelle={quelle} />}
    </div>
  );
}

function Befunde({ befunde, quelle }: { befunde: ProbeBefund[]; quelle: string | null }) {
  return (
    <ul className="m-0 flex list-none flex-col gap-1 p-0">
      {befunde.map((b) => (
        <li
          key={`${b.regel ?? ""}|${b.element ?? ""}|${b.text}`}
          className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-body text-critical"
        >
          <Icon name="warn" size={12} className="shrink-0 translate-y-px" />
          <span className="min-w-0 flex-1 text-text">{b.text}</span>
          {quelle && b.regel && <SourceChip quelle={{ dokument: quelle, regel: b.regel }} />}
        </li>
      ))}
    </ul>
  );
}

/** The interchange as rendered, one segment a line, with a copy button. */
function Wire({ text }: { text: string }) {
  const t = useT();
  const [kopiert, setKopiert] = useState(false);
  const zeilen = text.replaceAll("'", "'\n").trimEnd();
  return (
    <details className="group">
      <summary className="focus-ring cursor-default rounded-control text-callout font-medium text-text-2 marker:text-text-2">
        {t.quickLook.wire}
      </summary>
      <div className="mt-2 flex flex-col gap-2">
        <pre className="m-0 max-h-80 overflow-auto rounded-control bg-fill p-3 font-mono text-caption text-text">
          {zeilen}
        </pre>
        <div>
          <Button
            variant="secondary"
            onPress={() => {
              navigator.clipboard?.writeText(text).then(
                () => setKopiert(true),
                () => setKopiert(false),
              );
            }}
          >
            <Icon name={kopiert ? "check" : "copy"} size={14} />
            <span aria-live="polite">{kopiert ? t.quickLook.kopiert : t.quickLook.kopieren}</span>
          </Button>
        </div>
      </div>
    </details>
  );
}
