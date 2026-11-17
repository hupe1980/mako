import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { useId, useState } from "react";
import { Button as AriaButton } from "react-aria-components";
import { nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { lesen, RestAbgelehnt } from "../data/api/client";
import type { EinsdSettlementReceipt } from "../data/api/schema";
import { ServiceUnavailable } from "../data/mcp";
import { Angabe, AngabenListe } from "../design/ListDetail";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import type { Decimal } from "../format/brands";
import { formatBusinessDate } from "../format/date";
import { formatDecimal, formatEuro } from "../format/money";
import { eintrag, useT } from "../i18n";

function Fehler({ error, dienst }: { error: unknown; dienst: "einsd" | "accountingd" }) {
  const t = useT();
  return (
    <Notice
      tone={error instanceof RestAbgelehnt ? "refusal" : "failure"}
      title={error instanceof ServiceUnavailable ? nichtErreichbar(dienst) : t.lokation.abgelehnt}
    >
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

/** One settled month's statement, as einsd explains it: every position with its basis. */
function Erklaerung({ trId, s }: { trId: string; s: EinsdSettlementReceipt }) {
  const t = useT();
  const k = t.anlage;
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "rest", "einsd", "erklaerung", trId, s.billing_year, s.billing_month],
    queryFn: () =>
      lesen("einsd", "/svc/einsd/api/v1/anlagen/{tr_id}/settlements/{year}/{month}/erklaerung", {
        path: { tr_id: trId, year: s.billing_year, month: s.billing_month },
      }),
  });
  if (q.isError) return <Fehler error={q.error} dienst="einsd" />;
  if (!q.data) return <div className="skeleton h-24" />;
  return (
    <div className="flex flex-col gap-2">
      <p className="m-0 text-body text-text-2">{q.data.interpretation}</p>
      <ol aria-label={k.positionen} className="m-0 list-none p-0">
        {q.data.positions.map((p) => (
          <li
            key={`${p.description}${p.legal_basis}`}
            className="grid grid-cols-[minmax(0,1fr)_7rem_6rem_7rem] items-baseline gap-3 border-b border-separator py-2 text-body last:border-b-0"
          >
            <span className="flex flex-wrap items-baseline gap-2">
              {p.description}
              <SourceChip quelle={{ dokument: p.legal_basis }} />
            </span>
            <span className="text-right text-text-2 tabular-nums">
              {k.kwh(formatDecimal(p.kwh as Decimal))}
            </span>
            <span className="text-right text-text-2 tabular-nums">
              {k.ct(formatDecimal(p.rate_ct_kwh as Decimal))}
            </span>
            <span className="text-right tabular-nums">{formatEuro(p.eur as Decimal)}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/**
 * The Anlage page: einsd's facts about a plant with its Förderende large,
 * then its settled months as a strip — einsd's status, and paid where
 * accountingd's EEG payouts say so — each month opening einsd's statement.
 * Every figure is a service's.
 */
export function AnlageView() {
  const t = useT();
  const k = t.anlage;
  const { trId } = useParams({ from: "/anlage/$trId" });
  const { rolle } = useRolle();
  const streifenId = useId();
  const [monat, setMonat] = useState<string | null>(null);
  const anlage = useQuery({
    queryKey: [rolle, "rest", "einsd", "anlage", trId],
    queryFn: () => lesen("einsd", "/svc/einsd/api/v1/anlagen/{tr_id}", { path: { tr_id: trId } }),
  });
  const abrechnungen = useQuery({
    queryKey: [rolle, "rest", "einsd", "settlements", trId],
    queryFn: () =>
      lesen("einsd", "/svc/einsd/api/v1/anlagen/{tr_id}/settlements", {
        path: { tr_id: trId },
        query: { limit: 12 },
      }),
  });
  const auszahlungen = useQuery({
    queryKey: [rolle, "rest", "accountingd", "eeg-payouts", trId],
    queryFn: () =>
      lesen("accountingd", "/svc/accountingd/api/v1/eeg/payouts", {
        query: { tr_id: trId, limit: 24 },
      }),
  });
  const a = anlage.data;
  const bezahlt = new Set(
    (auszahlungen.data?.payouts ?? [])
      .filter((p) => p.settled_at)
      .map((p) => `${p.billing_year}-${p.billing_month}`),
  );
  // einsd lists the newest first; the strip reads left to right in time.
  const monate = [...(abrechnungen.data?.settlements ?? [])].reverse();
  const gewaehlt = monate.find((s) => s.id === monat) ?? null;
  return (
    <Page width="work">
      <Panel className="px-6 pt-5 pb-5">
        {anlage.isError && <Fehler error={anlage.error} dienst="einsd" />}
        {anlage.isPending && <div className="skeleton h-16" />}
        {a && (
          <div className="flex flex-wrap items-start gap-6">
            <div className="min-w-0 flex-1">
              <h1 className="m-0 text-large-title font-semibold">
                {k.titel(eintrag(k.erzeugungsart, a.erzeugungsart), formatDecimal(a.leistung_kwp))}
              </h1>
              <ul
                aria-label={t.prozessAnsicht.eckdaten}
                className="m-0 mt-1.5 flex list-none flex-wrap gap-x-3 p-0 text-body"
              >
                <li>{a.tr_id}</li>
                <li>MaLo {a.malo_id}</li>
                <li>{k.eeg(`${a.eeg_gesetz}`)}</li>
                <li>{k.inbetrieb(formatBusinessDate(a.inbetriebnahme))}</li>
                <li className={a.mastr_registriert ? undefined : "text-critical"}>
                  {a.mastr_registriert ? k.mastr(a.mastr_nummer ?? "") : k.ohneMastr}
                </li>
              </ul>
            </div>
            {a.foerderendedatum && (
              // biome-ignore lint/a11y/useSemanticElements: a labelled figure and its caption
              <div
                role="group"
                aria-label={k.foerderende}
                className="flex flex-col items-end gap-1"
              >
                <span className="text-title1 font-semibold tabular-nums">
                  {formatBusinessDate(a.foerderendedatum)}
                </span>
                <span className="text-caption text-text-2">{k.foerderende}</span>
              </div>
            )}
          </div>
        )}
      </Panel>

      <Panel className="overflow-hidden">
        <section aria-labelledby={streifenId} className="px-6 pt-5 pb-5">
          <h2 id={streifenId} className="m-0 mb-3 text-headline">
            {k.abrechnungen}
          </h2>
          {abrechnungen.isError && <Fehler error={abrechnungen.error} dienst="einsd" />}
          {abrechnungen.data && monate.length === 0 && (
            <p className="m-0 text-body text-text-2">{k.keineAbrechnung}</p>
          )}
          <ol
            aria-label={k.abrechnungen}
            className="m-0 grid list-none grid-cols-6 gap-1.5 p-0 md:grid-cols-12"
          >
            {monate.map((s) => {
              const paid = bezahlt.has(`${s.billing_year}-${s.billing_month}`);
              return (
                <li key={s.id} data-status={s.status} data-bezahlt={paid}>
                  <AriaButton
                    onPress={() => setMonat(monat === s.id ? null : s.id)}
                    aria-expanded={monat === s.id}
                    className={`focus-ring flex h-16 w-full flex-col items-start justify-between rounded-[10px] border px-2 py-1.5 text-left ${
                      monat === s.id ? "border-accent" : "border-separator"
                    } ${s.status === "error" ? "bg-critical-fill" : paid ? "bg-done-fill" : "bg-fill"}`}
                  >
                    <span className="text-caption text-text-2 tabular-nums">
                      {k.monat(`${s.billing_month}`.padStart(2, "0"), `${s.billing_year}`)}
                    </span>
                    <span className="text-caption font-semibold tabular-nums">
                      {s.settlement_eur
                        ? formatEuro(s.settlement_eur as Decimal)
                        : eintrag(k.status, s.status)}
                    </span>
                    <span className="sr-only">
                      {eintrag(k.status, s.status)}
                      {paid && ` · ${k.bezahlt}`}
                    </span>
                  </AriaButton>
                </li>
              );
            })}
          </ol>
          {gewaehlt && (
            <div className="mt-4 border-t border-separator pt-4">
              <Erklaerung trId={trId} s={gewaehlt} />
            </div>
          )}
        </section>
      </Panel>

      {a && (
        <Panel className="px-6 py-5">
          <AngabenListe>
            <Angabe name={k.verguetungsform}>
              {eintrag(k.verguetungsformen, a.verguetungsform)}
            </Angabe>
            <Angabe name={k.verguetungssatz}>{k.ct(formatDecimal(a.verguetungssatz_ct))}</Angabe>
            <Angabe name={k.direktvermarktung}>{a.direktvermarktung ? k.ja : k.nein}</Angabe>
            <Angabe name={k.zustand}>{eintrag(k.anlagenstatus, a.status)}</Angabe>
          </AngabenListe>
        </Panel>
      )}
    </Page>
  );
}
