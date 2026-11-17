import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useId, useState } from "react";
import { Button as AriaButton, Form, Input, Label, TextField } from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { ServiceUnavailable } from "../data/mcp";
import {
  anlegen,
  type Endpunkt,
  endpunkte,
  erneutSenden,
  MIT_ZUSTELLUNGEN,
  schluesselErneuern,
  testen,
  type ZustellDienst,
  ZustellungAbgelehnt,
  zeitraumErneut,
  zustellungen,
} from "../data/zustellung";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import type { Instant } from "../format/brands";
import { formatCount } from "../format/count";
import { berlinMitternacht, formatInstant, heuteBerlin, tageVerschoben } from "../format/date";
import { eintrag, useT } from "../i18n";

/** How long the old signing secret keeps working after a renewal (the service's maximum is 168 h). */
const UEBERLAPPUNG_STUNDEN = 24;

function Fehler({ error, dienst }: { error: unknown; dienst: ZustellDienst }) {
  const t = useT();
  const satz =
    error instanceof ZustellungAbgelehnt && error.code
      ? t.codes[error.code as keyof typeof t.codes]
      : undefined;
  return (
    <Notice
      tone={error instanceof ServiceUnavailable ? "failure" : "refusal"}
      title={
        error instanceof ServiceUnavailable
          ? nichtErreichbar(dienst)
          : (satz ?? t.zustellungen.abgelehnt)
      }
    >
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

/** A signing secret, shown this once with a copy button — the service keeps no copy it could show again. */
function Geheimnis({ wert }: { wert: string }) {
  const t = useT();
  const k = t.zustellungen;
  const [kopiert, setKopiert] = useState(false);
  return (
    <div className="flex flex-col gap-2 rounded-panel border border-soon/30 bg-soon-fill p-4">
      <p className="m-0 text-body font-semibold">{k.einmal}</p>
      <div className="flex flex-wrap items-center gap-3">
        <code className="min-w-0 flex-1 font-mono text-callout break-all">{wert}</code>
        <Button
          onPress={() => void navigator.clipboard?.writeText(wert).then(() => setKopiert(true))}
        >
          <Icon name={kopiert ? "check" : "copy"} size={14} />
          {kopiert ? t.nachrechnen.kopiert : k.kopieren}
        </Button>
      </div>
    </div>
  );
}

function Lieferungen({ dienst, e }: { dienst: ZustellDienst; e: Endpunkt }) {
  const t = useT();
  const k = t.zustellungen;
  const { rolle } = useRolle();
  const client = useQueryClient();
  const [ergebnis, setErgebnis] = useState<string | null>(null);
  const [geheim, setGeheim] = useState<string | null>(null);
  const [fehler, setFehler] = useState<unknown>(null);
  const q = useQuery({
    queryKey: [rolle, "rest", dienst, "deliveries", e.id],
    queryFn: () => zustellungen(dienst, e.id),
  });
  const versuch = async (f: () => Promise<unknown>) => {
    setFehler(null);
    try {
      await f();
      void client.invalidateQueries({ queryKey: [rolle, "rest", dienst] });
    } catch (x) {
      setFehler(x);
    }
  };
  const heute = heuteBerlin();
  const config = e.owner === "config";
  return (
    <div className="flex flex-col gap-3 border-t border-separator px-6 py-4">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          onPress={() =>
            void versuch(async () => {
              const r = await testen(dienst, e.id);
              setErgebnis(
                r.delivered
                  ? k.testOk(`${r.status ?? "—"}`, formatCount(r.latency_ms ?? 0))
                  : k.testFehler(r.error ?? `${r.status ?? ""}`),
              );
            })
          }
        >
          {k.testen}
        </Button>
        <Button
          onPress={() =>
            void versuch(async () => {
              await zeitraumErneut(
                dienst,
                e.id,
                berlinMitternacht(tageVerschoben(heute, -1)),
                berlinMitternacht(tageVerschoben(heute, 1)),
              );
              setErgebnis(k.zeitraumGesendet);
            })
          }
        >
          {k.zeitraum}
        </Button>
        {!config && (
          <Button
            onPress={() =>
              void versuch(async () => {
                const r = await schluesselErneuern(dienst, e.id, UEBERLAPPUNG_STUNDEN);
                setGeheim(r.secret);
              })
            }
          >
            {k.erneuern}
          </Button>
        )}
        {ergebnis && <span className="text-body text-text-2">{ergebnis}</span>}
      </div>
      {geheim && <Geheimnis wert={geheim} />}
      {fehler !== null && <Fehler error={fehler} dienst={dienst} />}
      {q.isError && <Fehler error={q.error} dienst={dienst} />}
      {q.data && q.data.items.length === 0 && (
        <p className="m-0 text-body text-text-2">{k.keineZustellung}</p>
      )}
      <ol aria-label={k.liste} className="m-0 list-none p-0">
        {q.data?.items.map((z) => (
          <li
            key={z.id}
            className="grid grid-cols-[9rem_minmax(0,1fr)_auto_auto_auto] items-center gap-3 border-b border-separator py-2 text-body last:border-b-0"
          >
            <span className="text-caption text-text-2 tabular-nums">
              {formatInstant(z.created_at as Instant)}
            </span>
            <span className="min-w-0 truncate font-mono text-callout">{z.event_type}</span>
            <span
              className={`rounded-full px-2 text-caption font-semibold ${
                z.status === "delivered"
                  ? "bg-done-fill text-done"
                  : z.status === "dead_lettered"
                    ? "bg-critical-fill text-critical"
                    : "bg-info-fill text-info"
              }`}
            >
              {eintrag(k.status, z.status)}
              {typeof z.last_status === "number" && ` · ${z.last_status}`}
            </span>
            <span className="text-caption text-text-2 tabular-nums">
              {typeof z.latency_ms === "number" ? k.ms(formatCount(z.latency_ms)) : ""}
            </span>
            <AriaButton
              onPress={() => void versuch(() => erneutSenden(dienst, z.id))}
              className="focus-ring rounded-[6px] px-2 text-caption text-accent underline underline-offset-2"
            >
              {k.erneut}
            </AriaButton>
          </li>
        ))}
      </ol>
    </div>
  );
}

/**
 * Zustellungen: each service's webhook endpoints, as the integrator API keeps
 * them — a new endpoint's signing secret shown once, a test event, the
 * deliveries with status and latency, a replay of one or of the last day. An
 * endpoint from the service's configuration is shown, not edited.
 */
export function ZustellungenView() {
  const t = useT();
  const k = t.zustellungen;
  const { rolle } = useRolle();
  const navigate = useNavigate({ from: "/zustellungen" });
  const { dienst: gewaehlt } = useSearch({ from: "/zustellungen" });
  const dienst: ZustellDienst = MIT_ZUSTELLUNGEN.find((d) => d === gewaehlt) ?? "processd";
  const [offen, setOffen] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [neu, setNeu] = useState<string | null>(null);
  const [fehler, setFehler] = useState<unknown>(null);
  const client = useQueryClient();
  const listeId = useId();
  const q = useQuery({
    queryKey: [rolle, "rest", dienst, "endpoints"],
    queryFn: () => endpunkte(dienst),
  });
  return (
    <Page width="work" title={k.titel} subtitle={k.untertitel}>
      <nav aria-label={k.dienste} className="flex flex-wrap gap-1.5">
        {MIT_ZUSTELLUNGEN.map((d) => (
          <AriaButton
            key={d}
            onPress={() => {
              setOffen(null);
              setNeu(null);
              navigate({ search: { dienst: d } });
            }}
            {...(d === dienst ? { "aria-current": "page" as const } : {})}
            className={`focus-ring h-8 rounded-full border px-3 text-body ${d === dienst ? "border-accent bg-[color-mix(in_srgb,var(--accent)_10%,transparent)]" : "border-separator data-[hovered]:bg-fill"}`}
          >
            {DIENST[d]}
          </AriaButton>
        ))}
      </nav>
      <Panel className="overflow-hidden">
        <section aria-labelledby={listeId}>
          <h2 id={listeId} className="m-0 px-6 pt-5 pb-3 text-headline">
            {k.endpunkte}
          </h2>
          {q.isError && (
            <div className="px-6 pb-4">
              <Fehler error={q.error} dienst={dienst} />
            </div>
          )}
          {q.data && q.data.items.length === 0 && (
            <p className="m-0 px-6 pb-4 text-body text-text-2">{k.keinEndpunkt}</p>
          )}
          <ul aria-label={k.endpunkte} className="m-0 list-none p-0">
            {q.data?.items.map((e) => (
              <li key={e.id} className="border-t border-separator">
                <AriaButton
                  onPress={() => setOffen(offen === e.id ? null : e.id)}
                  aria-expanded={offen === e.id}
                  className="focus-ring flex w-full flex-wrap items-baseline gap-3 px-6 py-3 text-left data-[hovered]:bg-fill"
                >
                  <span className="text-headline">{e.name}</span>
                  <span className="min-w-0 flex-1 truncate font-mono text-callout text-text-2">
                    {e.url}
                  </span>
                  {e.owner === "config" && (
                    <span className="rounded-full bg-fill px-2 text-caption text-text-2">
                      {k.ausKonfiguration}
                    </span>
                  )}
                  <span className={`text-caption ${e.active ? "text-done" : "text-critical"}`}>
                    {e.active ? k.aktiv : k.inaktiv}
                  </span>
                </AriaButton>
                {offen === e.id && <Lieferungen dienst={dienst} e={e} />}
              </li>
            ))}
          </ul>
        </section>
      </Panel>
      <Panel className="px-6 py-5">
        <h2 className="m-0 mb-3 text-headline">{k.anlegen}</h2>
        <Form
          onSubmit={(ev) => {
            ev.preventDefault();
            setFehler(null);
            anlegen(dienst, { name: name.trim(), url: url.trim() }).then(
              (r) => {
                setNeu(r.secret);
                setName("");
                setUrl("");
                void client.invalidateQueries({ queryKey: [rolle, "rest", dienst] });
              },
              (x: unknown) => setFehler(x),
            );
          }}
          className="flex flex-wrap items-end gap-3"
        >
          <TextField value={name} onChange={setName} isRequired className="flex flex-col gap-1">
            <Label className="text-caption text-text-2">{k.name}</Label>
            <Input className="focus-ring h-8 rounded-control border border-separator bg-surface px-3 text-body" />
          </TextField>
          <TextField
            value={url}
            onChange={setUrl}
            isRequired
            className="flex min-w-64 flex-1 flex-col gap-1"
          >
            <Label className="text-caption text-text-2">{k.url}</Label>
            <Input className="focus-ring h-8 rounded-control border border-separator bg-surface px-3 font-mono text-callout" />
          </TextField>
          <Button type="submit" variant="primary" isDisabled={!name.trim() || !url.trim()}>
            {k.anlegenKnopf}
          </Button>
        </Form>
        {neu && (
          <div className="mt-4">
            <Geheimnis wert={neu} />
          </div>
        )}
        {fehler !== null && (
          <div className="mt-4">
            <Fehler error={fehler} dienst={dienst} />
          </div>
        )}
      </Panel>
    </Page>
  );
}
