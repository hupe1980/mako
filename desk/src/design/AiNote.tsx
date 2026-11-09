import type { ReactNode } from "react";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/**
 * What a model wrote. agentd's text already opens with its disclosure sentence
 * (`MACHINE_GENERATED`, „AI-generated. "), and it is rendered as text, first —
 * the tint and the sparkle are the second signal, never the only one.
 */
export function AiNote({ text, footer }: { text: string; footer?: ReactNode }) {
  const t = useT();
  return (
    <section
      aria-label={t.aiNote.verfasst}
      className="squircle rounded-panel border border-ai-border bg-ai-fill p-4"
    >
      <div className="mb-2 flex items-center gap-1.5 text-caption font-semibold text-ai">
        <Icon name="sparkle" size={14} />
        {t.aiNote.agent}
      </div>
      <p className="m-0 whitespace-pre-line text-callout text-text">{text}</p>
      {footer && <div className="mt-3 border-t border-ai-border pt-3">{footer}</div>}
    </section>
  );
}
