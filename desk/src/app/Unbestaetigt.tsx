import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { call } from "../data/mcp";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { useT } from "../i18n";
import { type Unbestaetigt as Eintrag, unbestaetigt, vergessen } from "./holdQueue";
import { useRolle } from "./rollen";

/** The kinds of hold that are decisions; a hold of another kind needs no follow-up. */
const ENTSCHEIDUNGEN = ["freigabe/", "disput/", "pruefposten/", "buchung/", "gehalten/"];

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
      <span className="font-semibold">{e.label}</span> — {ausgang}
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
  const [liste, setListe] = useState(() => {
    const alle = unbestaetigt();
    for (const u of alle)
      if (!ENTSCHEIDUNGEN.some((p) => u.key.startsWith(p))) vergessen(u.schluessel);
    return alle.filter((u) => ENTSCHEIDUNGEN.some((p) => u.key.startsWith(p)));
  });
  if (liste.length === 0) return null;
  return (
    <section
      aria-label={k.titel}
      className="flex items-start gap-3 border-b border-separator bg-info-fill px-(--page-gutter) py-2 lg:px-(--page-gutter-wide)"
    >
      <Icon name="clock" size={14} className="mt-0.5 shrink-0 text-info" />
      <div className="min-w-0 flex-1">
        <p className="m-0 text-body font-semibold text-text">{k.titel}</p>
        <ul className="m-0 mt-0.5 list-none p-0">
          {liste.map((e) => (
            <Zeile key={e.schluessel} e={e} />
          ))}
        </ul>
      </div>
      <Button
        onPress={() => {
          for (const e of liste) vergessen(e.schluessel);
          setListe([]);
        }}
      >
        {k.verstanden}
      </Button>
    </section>
  );
}
