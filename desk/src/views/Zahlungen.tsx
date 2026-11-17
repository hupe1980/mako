import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { nichtErreichbar } from "../app/dienste";
import { useHold } from "../app/Holds";
import { useRolle } from "../app/rollen";
import { api, lesen, rest } from "../data/api/client";
import type {
  AccountingdGuthabenauszahlung,
  AccountingdUnzugeordneteZahlung,
} from "../data/api/schema";
import { ServiceUnavailable } from "../data/mcp";
import { ausgezahlt, ZahlungAbgelehnt, zuordnen } from "../data/zahlung";
import { Button } from "../design/Button";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import type { BusinessDate, Decimal } from "../format/brands";
import { formatBusinessDate, heuteBerlin } from "../format/date";
import { formatEuro } from "../format/money";
import { useT } from "../i18n";

function Fehler({ error }: { error: unknown }) {
  const t = useT();
  const satz =
    error instanceof ZahlungAbgelehnt && error.code
      ? t.codes[error.code as keyof typeof t.codes]
      : undefined;
  return (
    <Notice
      tone={error instanceof ServiceUnavailable ? "failure" : "refusal"}
      title={
        error instanceof ServiceUnavailable
          ? nichtErreichbar("accountingd")
          : (satz ?? t.zahlungen.abgelehnt)
      }
    >
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

/**
 * One payment nobody's account took: accountingd's best match for it — asked
 * of the service, never guessed — and one key to accept it, held for undo.
 */
function Zahlung({ z }: { z: AccountingdUnzugeordneteZahlung }) {
  const t = useT();
  const k = t.zahlungen;
  const { rolle } = useRolle();
  const hold = useHold();
  const client = useQueryClient();
  const [stand, setStand] = useState<"offen" | "gehalten" | "erledigt">("offen");
  const [fehler, setFehler] = useState<unknown>(null);
  const vorschlag = useQuery({
    queryKey: [rolle, "rest", "accountingd", "match", z.payment_id],
    queryFn: () =>
      rest(
        "accountingd",
        api.POST("/svc/accountingd/api/v1/payments/match", {
          body: {
            amount_ct: z.amount_ct,
            reference: z.reference,
            ...(z.end_to_end_id ? { end_to_end_id: z.end_to_end_id } : {}),
            value_date: z.value_date,
          },
        }),
      ),
  });
  const kandidat = vorschlag.data?.candidates?.[0];
  function annehmen() {
    if (!kandidat) return;
    setFehler(null);
    setStand("gehalten");
    hold(
      `zahlung/${z.payment_id}`,
      k.zugeordnet,
      async (v) => {
        try {
          await zuordnen(
            z.payment_id,
            { malo_id: kandidat.malo_id, lf_mp_id: kandidat.lf_mp_id },
            v,
          );
          setStand("erledigt");
          void client.invalidateQueries();
        } catch (e) {
          setFehler(e);
          setStand("offen");
        }
      },
      () => setStand("offen"),
    );
  }
  return (
    <li
      className="flex flex-col gap-2 border-b border-separator px-6 py-4 last:border-b-0"
      onKeyDown={(e) => {
        if (e.key.toLowerCase() === "a" && !e.metaKey && !e.ctrlKey && stand === "offen") {
          e.preventDefault();
          annehmen();
        }
      }}
    >
      <div className="flex flex-wrap items-baseline gap-3">
        <span className="text-headline tabular-nums">{formatEuro(z.amount_eur as Decimal)}</span>
        <span className="min-w-0 flex-1 truncate text-body">{z.reference}</span>
        <span className="text-caption text-text-2 tabular-nums">
          {formatBusinessDate(z.value_date as BusinessDate)}
          {z.iban_masked && ` · ${z.iban_masked}`}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-3 text-body">
        {vorschlag.isPending && <span className="text-text-2">{k.sucht}</span>}
        {vorschlag.isError && <span className="text-text-2">{k.keinVorschlag}</span>}
        {vorschlag.data && !kandidat && <span className="text-text-2">{k.keinVorschlag}</span>}
        {kandidat && (
          <span className="flex-1">{k.vorschlag(kandidat.malo_id, kandidat.confidence)}</span>
        )}
        {kandidat && stand === "offen" && (
          <Button variant="primary" onPress={annehmen} aria-keyshortcuts="A">
            {k.zuordnen}
          </Button>
        )}
        {stand === "gehalten" && <span className="text-text-2">{t.klaerfall.gehalten}</span>}
        {stand === "erledigt" && <span className="text-done">{k.zugeordnet}</span>}
      </div>
      {fehler !== null && <Fehler error={fehler} />}
    </li>
  );
}

/** A Guthaben due: „Ausgezahlt" records the payout on today's date, held for undo. */
function Guthaben({ g }: { g: AccountingdGuthabenauszahlung }) {
  const t = useT();
  const k = t.zahlungen;
  const hold = useHold();
  const client = useQueryClient();
  const [stand, setStand] = useState<"offen" | "gehalten" | "erledigt">("offen");
  const [fehler, setFehler] = useState<unknown>(null);
  return (
    <li className="flex flex-col gap-2 border-b border-separator px-6 py-4 last:border-b-0">
      <div className="flex flex-wrap items-baseline gap-3">
        <span className="text-headline tabular-nums">{formatEuro(g.amount_eur as Decimal)}</span>
        <span className="min-w-0 flex-1 text-body">
          {k.guthabenAus(g.rechnungsnummer, g.malo_id)}
        </span>
        <span
          className={`text-caption tabular-nums ${g.overdue ? "text-critical" : "text-text-2"}`}
        >
          {k.faellig(formatBusinessDate(g.due_on as BusinessDate))}
        </span>
        {stand === "offen" && (
          <Button
            onPress={() => {
              setFehler(null);
              setStand("gehalten");
              hold(
                `guthaben/${g.payout_id}`,
                k.ausgezahltGemeldet,
                async (v) => {
                  try {
                    await ausgezahlt(g.payout_id, { paid_on: heuteBerlin() }, v);
                    setStand("erledigt");
                    void client.invalidateQueries();
                  } catch (e) {
                    setFehler(e);
                    setStand("offen");
                  }
                },
                () => setStand("offen"),
              );
            }}
          >
            {k.ausgezahlt}
          </Button>
        )}
        {stand === "gehalten" && <span className="text-text-2">{t.klaerfall.gehalten}</span>}
        {stand === "erledigt" && <span className="text-done">{k.ausgezahltGemeldet}</span>}
      </div>
      {fehler !== null && <Fehler error={fehler} />}
    </li>
  );
}

/**
 * Zahlungen: the payments no account took, each with accountingd's proposal
 * and one key to accept it, and the Guthaben due for payout — all read and
 * written over accountingd's REST API, every amount accountingd's.
 */
export function ZahlungenView() {
  const t = useT();
  const k = t.zahlungen;
  const { rolle } = useRolle();
  const ohneId = useId();
  const guthabenId = useId();
  const zahlungen = useQuery({
    queryKey: [rolle, "rest", "accountingd", "unmatched"],
    queryFn: () =>
      lesen("accountingd", "/svc/accountingd/api/v1/payments/unmatched", { query: { limit: 50 } }),
  });
  const guthaben = useQuery({
    queryKey: [rolle, "rest", "accountingd", "credit-payouts"],
    queryFn: () =>
      lesen("accountingd", "/svc/accountingd/api/v1/credit-payouts", {
        query: { overdue: true, limit: 50 },
      }),
  });
  return (
    <Page width="reading" title={k.titel} subtitle={k.untertitel}>
      <Panel className="overflow-hidden">
        <section aria-labelledby={ohneId}>
          <h2 id={ohneId} className="m-0 px-6 pt-5 pb-3 text-headline">
            {k.ohneZuordnung}
          </h2>
          {zahlungen.isError && <Fehler error={zahlungen.error} />}
          {zahlungen.data && zahlungen.data.payments.length === 0 && (
            <p className="m-0 px-6 pb-5 text-body text-text-2">{k.keineZahlung}</p>
          )}
          <ol aria-label={k.ohneZuordnung} className="m-0 list-none p-0">
            {zahlungen.data?.payments.map((z) => (
              <Zahlung key={z.payment_id} z={z} />
            ))}
          </ol>
        </section>
      </Panel>
      <Panel className="overflow-hidden">
        <section aria-labelledby={guthabenId}>
          <h2 id={guthabenId} className="m-0 px-6 pt-5 pb-3 text-headline">
            {k.guthabenFaellig}
          </h2>
          {guthaben.isError && <Fehler error={guthaben.error} />}
          {guthaben.data && guthaben.data.payouts.length === 0 && (
            <p className="m-0 px-6 pb-5 text-body text-text-2">{k.keinGuthaben}</p>
          )}
          <ol aria-label={k.guthabenFaellig} className="m-0 list-none p-0">
            {guthaben.data?.payouts.map((g) => (
              <Guthaben key={g.payout_id} g={g} />
            ))}
          </ol>
        </section>
      </Panel>
    </Page>
  );
}
