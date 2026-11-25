import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { useId, useState } from "react";
import { nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import type { LineView } from "../data/gen/billingd";
import { call, ServiceUnavailable, type ToolOutput } from "../data/mcp";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import { Refusal } from "../design/Refusal";
import { SourceChip } from "../design/SourceChip";
import type { Decimal } from "../format/brands";
import { formatBusinessDate } from "../format/date";
import { formatEuro } from "../format/money";
import { eintrag, useT } from "../i18n";

import { InvoiceDelivery } from "./InvoiceDelivery";

type Nachrechnung = ToolOutput<"billingd", "recompute_invoice">;

/** billingd's amounts are decimal strings; shown through the one money formatter. */
const euro = (v: string | null) => (v === null ? null : formatEuro(v as Decimal));

/** The verdict first, as esales' Preis nachrechnen leads with it. */
function Urteil({ n }: { n: Nachrechnung }) {
  const t = useT();
  const k = t.nachrechnen;
  const id = useId();
  const v = n.verdict;
  // The invoice's amount leads; what is left to pay after the Abschläge follows in the totals.
  const summe =
    n.totals.find((s) => s.kind === "BRUTTO") ?? n.totals.find((s) => s.kind === "ZU_ZAHLEN");
  const titel = v.storniert ? k.storniert : v.matches ? k.stimmt : k.weichtAb;
  return (
    <section
      aria-labelledby={id}
      className={`squircle relative overflow-hidden rounded-panel border px-7 py-6 ${
        v.matches
          ? "border-[color-mix(in_srgb,var(--done)_25%,transparent)] bg-done-fill"
          : "border-[color-mix(in_srgb,var(--critical)_25%,transparent)] bg-critical-fill"
      }`}
    >
      <span
        aria-hidden="true"
        className={`absolute inset-y-0 left-0 w-1 ${v.matches ? "bg-done" : "bg-critical"}`}
      />
      <h2 id={id} className="m-0 flex items-center gap-2 text-large-title font-semibold">
        {titel}
        {v.matches && <Icon name="check" size={24} className="text-done" />}
      </h2>
      <p className="measure m-0 mt-2 text-body text-text">
        {v.matches
          ? k.stimmtText
          : v.mismatched_positions.length > 0
            ? k.weichtAbIn(v.mismatched_positions.map((p) => `${p}`))
            : k.weichtAbSonst}
        {v.seal_holds === false && <> {k.siegelGebrochen}</>}
        {v.seal_holds === true && <> {k.siegelGilt}</>}
      </p>
      {summe && (
        <p className="m-0 mt-5">
          <span className="block text-large-title font-semibold tabular-nums">
            {euro(summe.recomputed_eur)}
          </span>
          <span className="text-caption text-text-2">{eintrag(k.summe, summe.kind)}</span>
        </p>
      )}
    </section>
  );
}

/**
 * The page the customer recomputes the invoice on — billingd's link, shown
 * as text to copy into a letter or a reply, never opened from the Desk.
 */
function KundenLink({ link }: { link: string }) {
  const t = useT();
  const k = t.nachrechnen;
  const [kopiert, setKopiert] = useState(false);
  return (
    <Panel className="flex flex-wrap items-center gap-3 px-6 py-4">
      <span className="text-body text-text-2">{k.kundenLink}</span>
      <code className="min-w-0 flex-1 truncate font-mono text-callout text-text">{link}</code>
      <Button
        onPress={() => {
          void navigator.clipboard?.writeText(link).then(() => {
            setKopiert(true);
            setTimeout(() => setKopiert(false), 1500);
          });
        }}
      >
        <Icon name={kopiert ? "check" : "copy"} size={14} />
        {kopiert ? k.kopiert : k.kopieren}
      </Button>
    </Panel>
  );
}

function Zeile({ l }: { l: LineView }) {
  const t = useT();
  const k = t.nachrechnen;
  return (
    <li
      data-stimmt={l.matches}
      className="flex flex-col gap-1.5 border-b border-separator px-6 py-4 last:border-b-0"
    >
      <div className="flex items-baseline gap-3">
        <span className="text-caption text-text-2 tabular-nums">{l.position}</span>
        <span className="min-w-0 flex-1 text-headline text-text">{l.text}</span>
        {!l.matches && l.stated_eur && (
          <span className="text-body text-text-2 line-through tabular-nums">
            {euro(l.stated_eur)}
          </span>
        )}
        <span className={`text-headline tabular-nums ${l.matches ? "text-text" : "text-critical"}`}>
          {euro(l.recomputed_eur ?? l.stated_eur)}
        </span>
        <span className="sr-only">{l.matches ? k.zeileStimmt : k.zeileWeichtAb}</span>
      </div>
      {l.formula && <p className="m-0 pl-6 text-body text-text-2 tabular-nums">{l.formula}</p>}
      {(l.quantity_source || l.price_source) && (
        <dl className="m-0 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-0.5 pl-6 text-caption">
          {l.quantity_source && (
            <>
              <dt className="text-text-2">{k.menge}</dt>
              <dd className="m-0 text-text">
                {l.quantity_source}
                {l.estimated && <span className="ml-2 text-soon">{k.geschaetzt}</span>}
              </dd>
            </>
          )}
          {l.price_source && (
            <>
              <dt className="text-text-2">{k.preis}</dt>
              <dd className="m-0 text-text">{l.price_source}</dd>
            </>
          )}
        </dl>
      )}
      {l.rules.length > 0 && (
        <p className="m-0 flex flex-wrap gap-1.5 pl-6">
          {l.rules.map((r) => (
            <SourceChip key={r} quelle={{ dokument: r }} />
          ))}
        </p>
      )}
    </li>
  );
}

/**
 * Nachrechnen: billingd recomputes the invoice from what it was issued from
 * and states whether every line, total and the seal agree. The verdict leads;
 * each line follows with the quantity and price it came from, the rule and
 * the arithmetic — all billingd's, none computed here.
 */
export function NachrechnenView() {
  const t = useT();
  const k = t.nachrechnen;
  const { id } = useParams({ from: "/rechnung/$id" });
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "billingd", "recompute_invoice", id],
    queryFn: () => call("billingd", "recompute_invoice", { id }),
  });
  const n = q.data;
  return (
    <Page
      width="reading"
      title={k.titel}
      subtitle={
        n
          ? k.untertitel(
              n.rechnungsnummer,
              n.malo_id,
              `${formatBusinessDate(n.period_from)} – ${formatBusinessDate(n.period_to)}`,
            )
          : undefined
      }
    >
      {q.isPending && <div className="skeleton h-48" />}
      {q.isError &&
        (q.error instanceof ServiceUnavailable ? (
          <Notice tone="failure" title={nichtErreichbar("billingd")}>
            {q.error.message}
          </Notice>
        ) : (
          <Refusal title={k.nichtNachrechenbar} raw={q.error.message} />
        ))}
      {n && (
        <>
          <Urteil n={n} />
          {rolle === "lieferant" && (
            <InvoiceDelivery reference={n.rechnungsnummer} dispatchedAt={n.dispatched_at} />
          )}
          {n.public_link && <KundenLink link={n.public_link} />}
          <Panel className="overflow-hidden">
            <h2 className="m-0 px-6 pt-5 text-title2 font-semibold">{k.berechnetAus}</h2>
            <ol aria-label={k.berechnetAus} className="m-0 list-none p-0 pt-2">
              {n.lines.map((l) => (
                <Zeile key={l.position} l={l} />
              ))}
            </ol>
          </Panel>
          <Panel className="px-6 py-4">
            <dl className="m-0 grid grid-cols-[minmax(0,1fr)_max-content] gap-y-1.5 text-body">
              {n.totals.map((s) => (
                <div key={`${s.kind}${s.rate_percent ?? ""}`} className="contents">
                  <dt className="text-text-2">
                    {eintrag(k.summe, s.kind)}
                    {s.rate_percent && ` ${s.rate_percent} %`}
                  </dt>
                  <dd
                    className={`m-0 text-right tabular-nums ${s.matches ? "text-text" : "text-critical"}`}
                  >
                    {euro(s.recomputed_eur)}
                  </dd>
                </div>
              ))}
              <div className="contents">
                <dt className="text-text-2">{k.vorauszahlungen}</dt>
                <dd className="m-0 text-right tabular-nums">{euro(n.vorauszahlungen_eur)}</dd>
              </div>
            </dl>
          </Panel>
        </>
      )}
    </Page>
  );
}
