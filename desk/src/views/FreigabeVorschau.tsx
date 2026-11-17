import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useRolle } from "../app/rollen";
import { proben } from "../data/probe";
import { Notice } from "../design/Panel";
import { eintrag, useT } from "../i18n";
import type { Entscheidungsprobe } from "./entscheidungsprobe";
import { QuickLook } from "./QuickLook";

/**
 * What the decision sends, as the services render it: processd's verdict on
 * the decision, then the message makod's door builds from processd's body —
 * every segment under its AHB name, its findings beside it. A command makod
 * cannot rehearse shows processd's body as it is.
 */
export function FreigabeVorschau({ probe }: { probe: Entscheidungsprobe }) {
  const t = useT();
  const k = t.freigabeVorschau;
  const id = useId();
  const { rolle } = useRolle();
  const nachricht = useQuery({
    queryKey: [rolle, "makod", "probe", probe.command, probe.body_sha256],
    queryFn: () => proben(probe.command ?? "", probe.payload, probe.marktrolle ?? undefined),
    enabled: probe.urteil === "besteht" && Boolean(probe.command),
    retry: false,
  });
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2">
      <h4 id={id} className="m-0 text-caption font-semibold text-text-2">
        {k.titel}
      </h4>
      <p
        className={`m-0 text-body ${probe.urteil === "abgelehnt" ? "text-critical" : "text-text"}`}
      >
        {eintrag(k.urteil, probe.urteil)}
        {probe.grund && <> — {probe.grund.text}</>}
      </p>
      {nachricht.data?.nachrichten.map((n, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: one rehearsal's messages, in makod's order
        <QuickLook key={i} nachricht={n} />
      ))}
      {nachricht.isError && (
        <Notice tone="info" title={k.ohneDarstellung}>
          <pre className="m-0 font-mono text-callout break-words whitespace-pre-wrap">
            {JSON.stringify(probe.payload, null, 2)}
          </pre>
        </Notice>
      )}
    </section>
  );
}
