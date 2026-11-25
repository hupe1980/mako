import { useQueries } from "@tanstack/react-query";
import { abdruck, type Dokument } from "../data/api/dokument";
import { STAND } from "../data/api/stand";
import { accountingdOberflaeche } from "../data/gen/accountingd";
import { billingdOberflaeche } from "../data/gen/billingd";
import { edmdOberflaeche } from "../data/gen/edmd";
import { einsdOberflaeche } from "../data/gen/einsd";
import { invoicdOberflaeche } from "../data/gen/invoicd";
import { makodOberflaeche } from "../data/gen/makod";
import { marktdOberflaeche } from "../data/gen/marktd";
import { netzbilanzdOberflaeche } from "../data/gen/netzbilanzd";
import { processdOberflaeche } from "../data/gen/processd";
import { vertragdOberflaeche } from "../data/gen/vertragd";
import { type Service, werkzeuge } from "../data/mcp";
import { abweichungen } from "../data/oberflaeche";
import { Icon } from "../design/Icon";
import { useT } from "../i18n";
import { DIENST } from "./dienste";
import { driftKey, driftRestKey } from "./imDrift";

/** The fingerprint each service's snapshot had when the Desk was generated. */
const GENERIERT: Record<Service, Record<string, string>> = {
  accountingd: accountingdOberflaeche,
  billingd: billingdOberflaeche,
  edmd: edmdOberflaeche,
  einsd: einsdOberflaeche,
  invoicd: invoicdOberflaeche,
  makod: makodOberflaeche,
  marktd: marktdOberflaeche,
  netzbilanzd: netzbilanzdOberflaeche,
  processd: processdOberflaeche,
  vertragd: vertragdOberflaeche,
};

const DIENSTE = Object.keys(GENERIERT) as Service[];

/**
 * The drift banner: a service serving a typed tool the Desk was generated
 * against with another schema is named once, at the top, instead of failing a
 * read later. A service that does not answer or lists no tools is not drift —
 * the view reading it says it is unreachable.
 */
export function Drift() {
  const t = useT();
  const antworten = useQueries({
    queries: DIENSTE.map((service) => ({
      queryKey: driftKey(service),
      queryFn: async () => {
        const tools = await werkzeuge(service);
        return tools ? abweichungen(tools, GENERIERT[service]) : null;
      },
      // Asked again when the Desk regains focus — a deploy while it was open
      // shows here — and at most once a minute otherwise.
      staleTime: 60_000,
      refetchOnWindowFocus: true,
      retry: false,
    })),
  });
  // Each REST document the Desk was generated against, asked for as served:
  // another `info.version` or another document is drift as well.
  const rest = Object.keys(STAND) as (keyof typeof DIENST)[];
  const dokumente = useQueries({
    queries: rest.map((service) => ({
      queryKey: driftRestKey(service),
      queryFn: async () => {
        const res = await fetch(`/svc/${service}/api/v1/openapi.json`, {
          headers: { accept: "application/json" },
        });
        if (!res.ok) return null;
        const d = (await res.json()) as Dokument;
        // An answer that is no OpenAPI document says nothing about drift.
        if (typeof d.info?.version !== "string") return null;
        const stand = STAND[service];
        return stand && (d.info?.version !== stand.version || abdruck(d) !== stand.abdruck)
          ? [d.info?.version ?? ""]
          : [];
      },
      staleTime: 60_000,
      refetchOnWindowFocus: true,
      retry: false,
    })),
  });
  const abweichend = [
    ...new Set([
      ...DIENSTE.filter((_, i) => (antworten[i]?.data?.length ?? 0) > 0),
      ...rest.filter((_, i) => (dokumente[i]?.data?.length ?? 0) > 0),
    ]),
  ];
  if (abweichend.length === 0) return null;
  return (
    <div
      role="alert"
      className="flex items-start gap-2 border-b border-soon/30 bg-soon-fill px-(--page-gutter) py-2 text-body text-text lg:px-(--page-gutter-wide)"
    >
      <Icon name="warn" size={14} className="mt-0.5 shrink-0 text-soon" />
      <span>{t.drift.text(abweichend.map((s) => DIENST[s]).join(", "), abweichend.length)}</span>
    </div>
  );
}
