import { useQuery } from "@tanstack/react-query";
import { useRolle } from "../app/rollen";
import { invoiceDelivery } from "../data/invoiceDelivery";
import { Panel } from "../design/Panel";
import type { Instant } from "../format/brands";
import { formatInstant } from "../format/date";
import { eintrag, useT } from "../i18n";

/** Keep dispatch and the evidence from each delivery channel independently readable. */
export function InvoiceDelivery({
  reference,
  dispatchedAt,
}: {
  reference: string;
  dispatchedAt: Instant | null;
}) {
  const k = useT().billingWatch;
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "outputd", "invoice-delivery", reference],
    queryFn: ({ signal }) => invoiceDelivery(reference, signal),
    refetchInterval: 60_000,
    retry: false,
  });
  const channels = !q.isError && Array.isArray(q.data) ? q.data : [];
  const available = channels.filter(
    (d) =>
      ["PORTAL", "EMAIL", "POST"].includes(d.channel) && d.status === "DELIVERED" && d.delivered_at,
  );
  return (
    <Panel className="flex flex-col gap-3 px-6 py-4">
      <div>
        <h2 className="m-0 text-headline">{k.issuance}</h2>
        <p className="m-0 text-body">
          {dispatchedAt ? k.issuedAt(formatInstant(dispatchedAt)) : k.unissued}
        </p>
      </div>
      <div>
        <h3 className="m-0 text-headline">{k.availability}</h3>
        <div role="status" aria-atomic="true" className="text-body">
          {q.isPending
            ? k.loading
            : q.isError
              ? k.unavailable
              : q.data === "denied"
                ? k.denied
                : available.length === 0
                  ? k.noEvidence
                  : available.map((d) => (
                      <p key={d.channel} className="m-0">
                        {d.channel === "PORTAL"
                          ? k.portalAt(formatInstant(d.delivered_at as Instant))
                          : k.deliveredAt(
                              eintrag(k.channel, d.channel),
                              formatInstant(d.delivered_at as Instant),
                            )}
                        {d.channel === "PORTAL" && d.read_at && (
                          <> · {k.openedAt(formatInstant(d.read_at))}</>
                        )}
                      </p>
                    ))}
        </div>
        {channels.some((d) => !available.includes(d)) && (
          <ul className="mt-2 mb-0 list-none space-y-1 p-0 text-callout text-text-2">
            {channels
              .filter((d) => !available.includes(d))
              .map((d) => (
                <li key={d.channel}>
                  {eintrag(k.channel, d.channel, k.unknownChannel)} ·{" "}
                  {eintrag(k.state, d.status, k.unknownState)}
                  {d.status === "SENT" && d.first_sent_at && (
                    <> · {formatInstant(d.first_sent_at)}</>
                  )}
                </li>
              ))}
          </ul>
        )}
      </div>
    </Panel>
  );
}
