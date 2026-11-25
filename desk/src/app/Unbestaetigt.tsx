import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { call } from "../data/mcp";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { eintrag, useT } from "../i18n";
import { type Unbestaetigt as Eintrag, unbestaetigt, vergessen } from "./holdQueue";
import { ROLLEN, useRolle } from "./rollen";

/** The kinds of hold that are decisions; a hold of another kind needs no follow-up. */
const ENTSCHEIDUNGEN = [
  "klaerfall/",
  "freigabe/",
  "disput/",
  "pruefposten/",
  "buchung/",
  "gehalten/",
];

/** The journal describes the case. It cannot tie a transition to this request's key. */
function FallZeile({ id }: { id: string }) {
  const t = useT();
  const k = t.unbestaetigt;
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "get_klaerfall", id],
    queryFn: () => call("processd", "get_klaerfall", { id }),
    retry: false,
  });
  const candidate = q.data?.klaerfall;
  const fall =
    candidate?.id === id &&
    (!candidate.owner_role || candidate.owner_role.toUpperCase() === ROLLEN[rolle].marktrolle)
      ? candidate
      : null;
  return (
    <li className="text-body text-text">
      {fall ? (
        <Link to="/klaerfall/$id" params={{ id }} className="focus-ring text-accent underline">
          {k.fall} · {fall.message_ref}
        </Link>
      ) : (
        k.fall
      )}
      {" — "}
      {q.isPending ? k.fragt : k.fallUnbekannt}
      {fall && <> {k.fallStand(eintrag(t.klaerfall.zustand, fall.zustand))}</>}
    </li>
  );
}

function Zeile({ e }: { e: Eintrag }) {
  const t = useT();
  const k = t.unbestaetigt;
  const { rolle } = useRolle();
  const freigabe = e.key.startsWith("freigabe/") ? e.key.slice("freigabe/".length) : null;
  // processd states a Freigabe's outcome by its id; the other kinds' lists show it.
  const q = useQuery({
    queryKey: [rolle, "processd", "get_queue_entry", freigabe],
    queryFn: () => call("processd", "get_queue_entry", { id: freigabe ?? "" }),
    enabled: freigabe !== null,
    retry: false,
  });
  const ausgang = !freigabe
    ? k.inDerListe
    : q.isPending
      ? k.fragt
      : q.isError || !q.data
        ? k.unbekannt
        : k.status(q.data.status, q.data.decided_by);
  return (
    <li className="text-body text-text">
      <span className="font-semibold">
        {eintrag(t.aktionen.art, e.key.split("/")[0] ?? "", t.prozessAnsicht.entscheidung)}
      </span>{" "}
      — {ausgang}
    </li>
  );
}

/**
 * The decisions sent as the page last closed whose answer it did not read:
 * what the service says became of each, once, until the person has seen it.
 */
export function Unbestaetigt() {
  const t = useT();
  const k = t.unbestaetigt;
  const { rolle } = useRolle();
  const [liste, setListe] = useState(() => {
    const alle = unbestaetigt();
    for (const u of alle)
      if (!ENTSCHEIDUNGEN.some((p) => u.key.startsWith(p))) vergessen(u.schluessel);
    return alle.filter((u) => ENTSCHEIDUNGEN.some((p) => u.key.startsWith(p)));
  });
  const sichtbar = liste.filter((e) => !e.rolle || e.rolle === rolle);
  if (sichtbar.length === 0) return null;
  return (
    <section
      aria-label={k.titel}
      className="flex items-start gap-3 border-b border-separator bg-info-fill px-(--page-gutter) py-2 lg:px-(--page-gutter-wide)"
    >
      <Icon name="clock" size={14} className="mt-0.5 shrink-0 text-info" />
      <div className="min-w-0 flex-1">
        <p className="m-0 text-body font-semibold text-text">{k.titel}</p>
        <ul className="m-0 mt-0.5 list-none p-0">
          {sichtbar.map((e) =>
            e.key.startsWith("klaerfall/") ? (
              <FallZeile key={e.schluessel} id={e.key.slice("klaerfall/".length)} />
            ) : (
              <Zeile key={e.schluessel} e={e} />
            ),
          )}
        </ul>
      </div>
      <Button
        onPress={() => {
          for (const e of sichtbar) vergessen(e.schluessel);
          setListe(liste.filter((e) => !sichtbar.includes(e)));
        }}
      >
        {k.verstanden}
      </Button>
    </section>
  );
}
