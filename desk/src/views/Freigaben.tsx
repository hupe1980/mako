import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter, useSearch } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import {
  Button as AriaButton,
  GridList,
  GridListItem,
  Label,
  Radio,
  RadioGroup,
  TextArea,
  TextField,
} from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { ABLEHNEN, FREIGEBEN, NAECHSTE, UEBERNEHMEN, VORIGE } from "../app/keys";
import { ROLLEN, useRolle } from "../app/rollen";
import { type Schicksal, schicksal, type Vorschlag } from "../data/agent";
import { Abgelehnt, type Entscheidung, entscheiden, SchonEntschieden } from "../data/freigabe";
import type { Freigabe } from "../data/gen/processd";
import { call, ServiceUnavailable } from "../data/mcp";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { EmptyState, Notice, Panel } from "../design/Panel";
import type { Instant } from "../format/brands";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";
import { useVorschlaege } from "../inbox/quellen";
import { Fragen, VorschlagKarte } from "./Fragen";
import { Verlauf } from "./Verlauf";

const ROW =
  "focus-ring flex cursor-default items-center gap-3 border-b border-separator px-4 py-3 outline-none last:border-b-0 data-[focused]:bg-fill data-[hovered]:bg-fill data-[selected]:bg-fill-strong";

const ART: Record<string, string> = {
  gehalten: "Antwort steht fest",
  eskaliert: "Antwort offen",
};

function useFreigaben() {
  const { rolle } = useRolle();
  const marktrolle = ROLLEN[rolle].marktrolle;
  return useQuery({
    queryKey: [rolle, "processd", "list_pending_approvals", marktrolle],
    queryFn: () => call("processd", "list_pending_approvals", { marktrolle }),
    refetchInterval: 30_000,
  });
}

/** The count the sidebar shows beside „Freigaben" — processd's, not a page's. */
export function useFreigabenZahl(): number | undefined {
  return useFreigaben().data?.count;
}

/** Where a proposal came from, so sending it can say whether it was changed. */
interface Herkunft {
  run: string;
  v: Vorschlag;
}

/**
 * The decision on one Freigabe — action, code, Bemerkung, the confirm — with
 * the assistant beside it. Freigaben and Heute both decide through it.
 */
export function Entscheiden({
  f,
  onDone,
  sofort = false,
}: {
  f: Freigabe;
  onDone: () => void;
  /** Take the waiting proposal into the form as soon as it is there (`U` on a Heute row). */
  sofort?: boolean;
}) {
  const [aktion, setAktion] = useState<"freigeben" | "ablehnen" | null>(null);
  const [code, setCode] = useState<string | null>(null);
  const [bemerkung, setBemerkung] = useState("");
  const [bestaetigen, setBestaetigen] = useState(false);
  const [herkunft, setHerkunft] = useState<Herkunft | null>(null);
  // processd's policy refused this person a decision once: the reason stands
  // where the controls were, for every Freigabe, for the rest of the session.
  const verweigert = useVerweigert({ dienst: "processd", aktion: "entscheiden" });
  // The proposal agentd computed when the Freigabe was queued — read, never
  // asked for: nobody waits on a model here.
  const wartend = useVorschlaege({ freigabe: [f.id] }).freigabe.get(f.id);
  const wartet = wartend?.vorschlag ? { v: wartend.vorschlag, run: wartend.run } : null;

  // processd states, per action, the codes pressing it needs — none where the
  // answer goes as held — by the same rule it checks the code against.
  const codes =
    aktion === "ablehnen" ? f.codes_ablehnen : aktion === "freigeben" ? f.codes_freigeben : null;
  const brauchtCode = Boolean(codes);
  const vollstaendig = aktion !== null && (!brauchtCode || code !== null);

  const senden = useMutation({
    mutationFn: async () => {
      if (!aktion) return;
      const e: Entscheidung = { aktion };
      if (brauchtCode && code && codes) {
        e.antwort_code = code;
        e.antwort_tree = codes.ebd;
      }
      if (bemerkung.trim()) e.bemerkung = bemerkung.trim();
      await entscheiden(f.id, e);
      if (herkunft) {
        const unveraendert =
          herkunft.v.aktion === aktion && (herkunft.v.antwortcode ?? null) === (code ?? null);
        const was: Schicksal = unveraendert ? "sent" : "edited";
        // The decision went out; the record of what became of the proposal
        // is the assistant's quality signal and never blocks it.
        await schicksal(herkunft.run, was).catch(() => undefined);
      }
    },
    onSuccess: onDone,
  });

  // A and R choose the action from the keyboard; the code and the confirm
  // follow with the arrow keys and ↵, as everywhere else.
  useEffect(() => {
    if (verweigert) return;
    const onKey = (e: KeyboardEvent) => {
      if (UEBERNEHMEN.match?.(e) && wartet) {
        e.preventDefault();
        uebernehmen(wartet.v, wartet.run);
        return;
      }
      const a =
        FREIGEBEN.match?.(e) && f.kann_freigeben
          ? "freigeben"
          : ABLEHNEN.match?.(e) && f.kann_ablehnen
            ? "ablehnen"
            : null;
      if (!a) return;
      e.preventDefault();
      setAktion(a);
      setCode(null);
      setBestaetigen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function uebernehmen(v: Vorschlag, run: string) {
    // agentd answers a Freigabe only with its two actions; anything else is
    // not this form's to take.
    if (v.aktion !== "freigeben" && v.aktion !== "ablehnen") return;
    setHerkunft({ run, v });
    setAktion(v.aktion);
    setCode(v.antwortcode);
    setBestaetigen(false);
  }

  // `U` on the Heute row opened this form: take the proposal once it is read.
  const genommen = useRef(false);
  useEffect(() => {
    if (!sofort || genommen.current || !wartet) return;
    genommen.current = true;
    uebernehmen(wartet.v, wartet.run);
  });

  if (verweigert) {
    return (
      <>
        <Panel className="flex flex-col gap-4 p-5">
          <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
          <Notice tone="refusal" title="Die Prozessüberwachung lässt Sie hier nicht entscheiden">
            {verweigert}
          </Notice>
        </Panel>
        <Fragen anker={{ freigabe: f.id }} />
      </>
    );
  }

  return (
    <>
      <Panel className="flex flex-col gap-4 p-5">
        <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
        {wartet && !herkunft && wartend && (
          <VorschlagKarte
            v={wartet.v}
            run={wartet.run}
            disclosure={wartend.disclosure}
            onUebernehmen={uebernehmen}
          />
        )}
        {!wartet && wartend?.status === "running" && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> Der Assistent bereitet einen Vorschlag vor …
          </p>
        )}
        {herkunft && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> Vom Assistenten vorgeschlagen — prüfen, dann senden.
          </p>
        )}
        <RadioGroup
          aria-label="Entscheidung"
          value={aktion}
          onChange={(v) => {
            setAktion(v as "freigeben" | "ablehnen");
            setCode(null);
            setBestaetigen(false);
          }}
          orientation="horizontal"
          className="flex gap-2"
        >
          {(["freigeben", "ablehnen"] as const).map((a) => (
            <Radio
              key={a}
              value={a}
              isDisabled={a === "freigeben" ? !f.kann_freigeben : !f.kann_ablehnen}
              className="focus-ring flex h-9 cursor-default items-center rounded-control border border-separator px-4 text-body font-medium text-text-2 data-[disabled]:opacity-40 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:bg-[color-mix(in_srgb,var(--accent)_10%,transparent)] data-[selected]:text-accent"
            >
              {a === "freigeben" ? "Freigeben" : "Ablehnen"}
            </Radio>
          ))}
        </RadioGroup>

        {aktion === "freigeben" && f.art === "gehalten" && f.antwort && (
          <p className="m-0 text-callout text-text">
            Gesendet wird die festgehaltene Antwort:{" "}
            <span className="font-mono">{f.antwort.code}</span>
            {f.antwort.bedeutung && <> — {f.antwort.bedeutung}</>}
          </p>
        )}

        {brauchtCode && codes && (
          <RadioGroup
            value={code}
            onChange={(c) => {
              setCode(c);
              setBestaetigen(false);
            }}
            className="flex flex-col gap-1.5"
          >
            <Label className="text-caption text-text-2">
              Antwortcode aus <span className="font-mono">{codes.ebd}</span>
            </Label>
            {codes.codes.map((c) => (
              <Radio
                key={c.code}
                value={c.code}
                className="focus-ring flex cursor-default items-start gap-3 rounded-[10px] border border-separator px-3 py-2 data-[hovered]:bg-fill data-[selected]:border-accent"
              >
                <span className="font-mono text-body font-semibold text-text">{c.code}</span>
                <span className="text-body text-text-2">{c.bedeutung}</span>
              </Radio>
            ))}
          </RadioGroup>
        )}

        {aktion && (
          <TextField value={bemerkung} onChange={setBemerkung} className="flex flex-col gap-1">
            <Label className="text-caption text-text-2">Bemerkung (optional)</Label>
            <TextArea
              rows={2}
              maxLength={512}
              className="focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none"
            />
          </TextField>
        )}

        {senden.isError &&
          !(senden.error instanceof Verweigert) &&
          (senden.error instanceof SchonEntschieden ? (
            <Notice tone="info" title="Schon entschieden">
              {senden.error.decidedBy ?? "Jemand"} hat diese Freigabe
              {senden.error.decidedAt
                ? ` am ${formatInstant(senden.error.decidedAt as Instant)} Uhr`
                : ""}{" "}
              {senden.error.status === "Rejected" ? "abgelehnt" : "freigegeben"}. Ihre Entscheidung
              wurde nicht gesendet.
            </Notice>
          ) : (
            <Notice
              tone="refusal"
              title="Die Prozessüberwachung hat die Entscheidung nicht angenommen"
            >
              {senden.error instanceof Abgelehnt
                ? senden.error.message
                : nichtErreichbar("processd")}
            </Notice>
          ))}

        {!bestaetigen ? (
          <div className="flex justify-end">
            <AriaButton
              isDisabled={!vollstaendig}
              onPress={() => setBestaetigen(true)}
              className="focus-ring h-9 rounded-control bg-accent px-4 text-body font-semibold text-on-accent data-[disabled]:opacity-40"
            >
              {aktion === "ablehnen" ? "Ablehnen …" : "Freigeben …"}
            </AriaButton>
          </div>
        ) : (
          <div
            role="alertdialog"
            aria-label="Senden bestätigen"
            className="flex flex-wrap items-center gap-3 rounded-[10px] bg-fill px-3 py-2"
          >
            <p className="m-0 flex-1 text-body text-text">
              Das sendet die Antwort als Marktnachricht an den Marktpartner — sie lässt sich nicht
              zurückholen.
            </p>
            <AriaButton
              onPress={() => setBestaetigen(false)}
              className="focus-ring h-8 rounded-control px-3 text-body text-text-2 data-[hovered]:bg-fill-strong"
            >
              Abbrechen
            </AriaButton>
            <AriaButton
              autoFocus
              isDisabled={senden.isPending}
              onPress={() => senden.mutate()}
              className="focus-ring h-8 rounded-control bg-accent px-3 text-body font-semibold text-on-accent data-[disabled]:opacity-40"
            >
              {senden.isPending ? "Sendet …" : "Jetzt senden"}
            </AriaButton>
          </div>
        )}
      </Panel>
      <Fragen anker={{ freigabe: f.id }} onVorschlag={uebernehmen} />
    </>
  );
}

function Detail({ f }: { f: Freigabe }) {
  const client = useQueryClient();
  const router = useRouter();
  const [erledigt, setErledigt] = useState(false);
  if (erledigt) {
    return (
      <Panel>
        <EmptyState title="Entschieden.">
          Die Prozessüberwachung hat die Antwort an die Marktkommunikation gegeben.
        </EmptyState>
      </Panel>
    );
  }
  return (
    <div className="flex flex-col gap-4">
      <Panel className="flex flex-col gap-3 p-5">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <p className="m-0 text-caption font-semibold tracking-wide text-text-2 uppercase">
              {ART[f.art] ?? f.art} · PID <span className="font-mono">{f.pid}</span>
            </p>
            <h2 className="m-0 mt-1 text-title font-semibold">{f.grund}</h2>
          </div>
          <span className="flex flex-col items-end gap-1 text-caption text-text-2">
            entscheiden bis
            <FristUhr due={f.expires_at} />
          </span>
        </div>
        {f.malo_id && (
          <AriaButton
            onPress={() => router.navigate({ to: "/prozess", search: { key: f.malo_id ?? "" } })}
            className="focus-ring inline-flex h-7 w-fit items-center gap-1 rounded-full border border-separator px-3 text-body text-text data-[hovered]:bg-fill"
          >
            MaLo <span className="font-mono">{f.malo_id}</span>
            <Icon name="chevron" size={12} />
          </AriaButton>
        )}
      </Panel>
      <Entscheiden
        f={f}
        onDone={() => {
          setErledigt(true);
          void client.invalidateQueries({ queryKey: ["processd", "list_pending_approvals"] });
          void client.invalidateQueries();
        }}
      />
      <Panel className="overflow-hidden">
        <Verlauf processId={f.process_id} deadlines={[]} />
      </Panel>
    </div>
  );
}

/**
 * Freigaben: the decisions processd held for a person — the earliest Frist
 * first, as processd orders them. Each opens with its facts, the codes its
 * Entscheidungsbaum allows, the process's Verlauf and the assistant; nothing
 * is sent until the person confirms.
 */
export function FreigabenView() {
  const router = useRouter();
  const { id } = useSearch({ from: "/freigaben" });
  const list = useFreigaben();
  const rows = list.data?.freigaben ?? [];
  const selected = rows.find((f) => f.id === id) ?? rows[0];
  const vorschlaege = useVorschlaege({ freigabe: rows.slice(0, 100).map((f) => f.id) }).freigabe;

  // J and K walk the queue in processd's order.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const step = NAECHSTE.match?.(e) ? 1 : VORIGE.match?.(e) ? -1 : 0;
      if (!step || rows.length === 0) return;
      e.preventDefault();
      const at = Math.max(
        0,
        rows.findIndex((f) => f.id === selected?.id),
      );
      const next = rows[Math.min(rows.length - 1, Math.max(0, at + step))];
      if (next) router.navigate({ to: "/freigaben", search: { id: next.id } });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [rows, selected?.id, router]);

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Freigaben</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Was die {DIENST.processd} einer Person zur Entscheidung vorlegt — die nächste Frist
          zuerst.
        </p>
      </header>
      {list.isError && (
        <Notice
          tone="failure"
          title={
            list.error instanceof ServiceUnavailable
              ? nichtErreichbar(list.error.service)
              : "Die Freigaben ließen sich nicht lesen"
          }
        >
          {list.error.message}
        </Notice>
      )}
      {list.isPending && <div className="skeleton h-40" />}
      {list.data && rows.length === 0 && (
        <Panel>
          <EmptyState title="Nichts zu entscheiden.">
            Die {DIENST.processd} hat alles selbst beantwortet.
          </EmptyState>
        </Panel>
      )}
      {selected && (
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
          <section
            aria-label="Offene Freigaben"
            className="flex flex-col gap-2 lg:sticky lg:top-20 lg:self-start"
          >
            <Panel className="overflow-hidden">
              <GridList
                aria-label="Offene Freigaben"
                selectionMode="single"
                disallowEmptySelection
                selectedKeys={[selected.id]}
                onSelectionChange={(keys) => {
                  const next = [...keys][0];
                  if (next) router.navigate({ to: "/freigaben", search: { id: String(next) } });
                }}
                className="outline-none"
              >
                {rows.map((f) => (
                  <GridListItem key={f.id} id={f.id} textValue={f.grund} className={ROW}>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-body font-medium text-text">
                        {f.grund}
                      </span>
                      <span className="block text-caption text-text-2">
                        {ART[f.art] ?? f.art} · PID <span className="font-mono">{f.pid}</span>
                        {f.malo_id && (
                          <>
                            {" · "}
                            <span className="font-mono">{f.malo_id}</span>
                          </>
                        )}
                      </span>
                    </span>
                    {vorschlaege.get(f.id)?.vorschlag && (
                      <span title="Vorschlag des Assistenten bereit" className="text-ai">
                        <Icon name="sparkle" size={14} />
                        <span className="sr-only">Vorschlag bereit</span>
                      </span>
                    )}
                    <FristUhr due={f.expires_at} />
                  </GridListItem>
                ))}
              </GridList>
            </Panel>
            {list.data?.saturated && (
              <p className="m-0 px-1 text-caption text-text-2">
                Die ersten {formatCount(rows.length)} — mehr liegen an.
              </p>
            )}
          </section>
          <Detail key={selected.id} f={selected} />
        </div>
      )}
    </div>
  );
}
