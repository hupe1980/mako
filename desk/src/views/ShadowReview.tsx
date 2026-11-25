import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useState } from "react";
import { useRolle } from "../app/rollen";
import { api, rest } from "../data/api/client";
import { type Antwort, lesen, RestAbgelehnt } from "../data/api/lesen";
import { Button } from "../design/Button";
import { ListDetail } from "../design/ListDetail";
import { ListRow } from "../design/ListRow";
import { Page } from "../design/Page";
import { formatInstant } from "../format/date";
import { useT } from "../i18n";

const path = "/svc/processd/api/v1/shadow/review";
type Review = Antwort<typeof path>;
type Row = Review["cohort"]["rows"][number];

function finding(row: Row) {
  const a = row.assessment;
  if (!a) return "unassessed";
  return a.timing === "late" ||
    a.actual.pid !== a.expected.pid ||
    a.actual.code !== a.expected.code ||
    a.actual.tree !== a.expected.tree
    ? "different"
    : "matched";
}

function Evidence({ row, data, reload }: { row: Row; data: Review; reload: () => Promise<void> }) {
  const t = useT().shadow;
  const a = row.assessment;
  const previous = data.resolutions.find((r) => r.occurrence === row.id);
  const closed = previous?.generation === data.generation;
  const [reason, setReason] = useState("");
  const [reference, setReference] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await rest(
        "processd",
        api.POST("/svc/processd/api/v1/shadow/review/{id}/resolve", {
          params: { path: { id: row.id } },
          body: {
            generation: data.generation,
            revision: data.revision,
            evidence: reference,
            reason,
          },
        }),
      );
      await reload();
    } catch (failure) {
      const code = failure instanceof RestAbgelehnt ? failure.code : undefined;
      setError(
        code === "shadow_revision_changed"
          ? t.stale
          : code === "shadow_forbidden"
            ? t.denied
            : code === "shadow_already_resolved"
              ? t.already
              : code === "shadow_reason_required"
                ? t.required
                : t.failed,
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="flex flex-col gap-4 p-5">
      <h2 className="text-title font-semibold">{t[finding(row)]}</h2>
      {a ? (
        <>
          <table className="w-full border-collapse text-left text-body [&_th]:px-2 [&_td]:px-2 [&_th]:py-2 [&_td]:py-2">
            <caption className="sr-only">{t.comparison}</caption>
            <thead className="border-b border-separator text-text-2">
              <tr>
                <td />
                <th scope="col">{t.expected}</th>
                <th scope="col">{t.actual}</th>
              </tr>
            </thead>
            <tbody>
              {(["pid", "tree", "code"] as const).map((k) => (
                <tr
                  key={k}
                  className={
                    a.expected[k] !== a.actual[k]
                      ? "border-b border-separator bg-fill"
                      : "border-b border-separator"
                  }
                >
                  <th scope="row">{t[k]}</th>
                  <td className="font-mono tabular-nums">{a.expected[k]}</td>
                  <td>
                    <strong className="font-mono">{a.actual[k]}</strong>{" "}
                    {a.expected[k] !== a.actual[k] && (
                      <span className="text-caption font-semibold text-accent">{t.different}</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-body [&_dt]:text-text-2">
            <dt>{t.step}</dt>
            <dd>{t.rule(a.pruefschritt, a.release, a.ebd_revision)}</dd>
            <dt>{t.due}</dt>
            <dd>{formatInstant(a.deadline.due_at)}</dd>
            <dt>{t.timing}</dt>
            <dd>{a.timing === "late" ? t.late : a.timing === "on_time" ? t.on_time : t.unknown}</dd>
          </dl>
        </>
      ) : (
        <p>{t.missing}</p>
      )}
      <details>
        <summary className="focus-ring cursor-pointer">{t.references}</summary>
        <dl className="break-all text-body text-text-2">
          <dt>{t.receipt}</dt>
          <dd>{row.request_sha256}</dd>
          {a && (
            <>
              <dt>{t.capture}</dt>
              <dd>{a.answer_sha256}</dd>
            </>
          )}
          {a && (
            <>
              <dt>{t.facts}</dt>
              <dd>{a.facts_fingerprint}</dd>
              <dt>{t.expectation}</dt>
              <dd>{a.expected_fingerprint}</dd>
              {a.incumbent_received_at && (
                <>
                  <dt>{t.received}</dt>
                  <dd>{formatInstant(a.incumbent_received_at)}</dd>
                </>
              )}
            </>
          )}
        </dl>
      </details>
      {previous && (
        <section>
          <h3>
            {closed
              ? previous.disposition === "mako_korrigiert"
                ? t.corrected
                : t.accepted
              : t.reopened}
          </h3>
          <p>
            {previous.actor} · {formatInstant(previous.recorded_at)}
          </p>
          <p>{previous.reason}</p>
          <p>{previous.evidence}</p>
        </section>
      )}
      {data.can_resolve &&
        !closed &&
        (finding(row) === "different" || data.corrections.includes(row.id)) && (
          <form
            className="flex flex-col items-start gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <label>
              {t.reason}
              <input
                required
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                className="focus-ring block rounded-control border border-separator p-2"
              />
            </label>
            <label>
              {t.evidence}
              <input
                required
                value={reference}
                onChange={(e) => setReference(e.target.value)}
                className="focus-ring block rounded-control border border-separator p-2"
              />
            </label>
            <Button type="submit" isDisabled={busy || !reason.trim() || !reference.trim()}>
              {data.corrections.includes(row.id) ? t.fix : t.save}
            </Button>
          </form>
        )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

export function ShadowReviewView() {
  const { rolle } = useRolle();
  const t = useT().shadow;
  const q = useQuery({
    queryKey: [rolle, "processd", "shadow-review"],
    queryFn: () => lesen("processd", path, {}),
    enabled: rolle === "netz",
    retry: false,
  });
  const { id: selected } = useSearch({ from: "/schattenbetrieb" });
  const navigate = useNavigate({ from: "/schattenbetrieb" });
  const [notice, setNotice] = useState("");
  const data = q.data;
  async function reload(message: string) {
    const result = await q.refetch();
    setNotice(result.isError ? t.failed : message);
  }
  if (rolle !== "netz" || q.isError || !data) {
    const denied =
      rolle !== "netz" || (q.error instanceof RestAbgelehnt && [401, 403].includes(q.error.status));
    return (
      <Page width="reading" title={t.title}>
        <p role="status">{denied ? t.denied : q.isError ? t.unavailable : t.loading}</p>
        {q.isError && <Button onPress={() => void reload(t.refreshed)}>{t.refresh}</Button>}
      </Page>
    );
  }
  const rows = data.cohort.rows;
  const closed = new Set(
    data.resolutions.filter((r) => r.generation === data.generation).map((r) => r.occurrence),
  );
  const corrections = new Set(data.corrections);
  const status = (r: Row) =>
    closed.has(r.id) ? "closed" : corrections.has(r.id) ? "checkCorrection" : finding(r);
  const row =
    rows.find((r) => r.id === selected) ??
    rows.find((r) => !closed.has(r.id) && (finding(r) !== "matched" || corrections.has(r.id))) ??
    rows[0];
  return (
    <Page
      width="work"
      title={t.title}
      subtitle={t.counts(
        data.assessed,
        data.eligible,
        data.open_differences,
        data.timing_unassessed,
      )}
      trailing={<Button onPress={() => void reload(t.refreshed)}>{t.refresh}</Button>}
    >
      <div className="rounded-panel border border-separator bg-surface p-4">
        <strong className="text-title2">{data.ready ? t.ready : t.blocked}</strong>
        <p className="mt-1 text-body text-text-2">
          {t.window(
            data.cohort.nb,
            formatInstant(data.cohort.start),
            formatInstant(data.cohort.end),
          )}
        </p>
        <p>
          {t.days(data.cohort.complete_werktage, data.required_werktage)} · {t.revision}{" "}
          {data.generation}
        </p>
      </div>
      <p className="text-body text-text-2">
        {t.agreed} {data.assessed - data.different} · {t.diff} {data.different} · {t.unknownCount}{" "}
        {data.unassessed}
      </p>
      <p role="status" className="empty:sr-only">
        {notice}
      </p>
      {row ? (
        <ListDetail
          label={t.rows}
          kopf={<h2>{t.rows}</h2>}
          selected={row.id}
          onSelect={(id) => void navigate({ search: { id }, replace: true })}
          liste={rows.map((r) => (
            <ListRow
              key={r.id}
              id={r.id}
              textValue={`${t.row} ${r.id.slice(0, 12)} ${t[status(r)]}`}
              title={`${t.row} ${r.id.slice(0, 12)}`}
              subtitle={t[status(r)]}
            />
          ))}
          detail={
            <Evidence
              key={`${row.id}-${data.generation}`}
              row={row}
              data={data}
              reload={() => reload(t.saved)}
            />
          }
          angaben={null}
        />
      ) : (
        <p>{t.empty}</p>
      )}
    </Page>
  );
}
