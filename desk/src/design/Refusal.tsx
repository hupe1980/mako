import type { ReactNode } from "react";
import { refusalCode } from "../app/codes";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/**
 * A service said no. The catalogue's sentence for the service's stable
 * `code` is what the operator reads; the service's own text — written for a
 * developer — waits under „Details". One action, the way on.
 */
export function Refusal({
  title,
  raw,
  action,
}: {
  title: string;
  /** The service's text as it came, `code: sentence` where it states a code. */
  raw: string;
  action?: ReactNode;
}) {
  const t = useT();
  const { sentence } = refusalCode(raw);
  return (
    <div role="status" className="flex flex-col items-start gap-2 py-2">
      <p className="m-0 text-title3 font-semibold text-text">{title}</p>
      <p className="measure m-0 text-body text-text-2">{sentence ?? t.refusal.unknown}</p>
      <details className="group">
        <summary className="focus-ring inline-flex cursor-default list-none items-center gap-1 rounded-[6px] text-callout text-text-2 hover:text-text [&::-webkit-details-marker]:hidden">
          <Icon name="chevron" size={11} className="transition-transform group-open:rotate-90" />
          {t.refusal.details}
        </summary>
        <p className="measure m-0 mt-1.5 font-mono text-callout break-words text-text-2">{raw}</p>
      </details>
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
