import type { ReactNode } from "react";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/**
 * What a model wrote. The text opens with the Desk's disclosure sentence
 * (`app/disclosure.ts` puts it where agentd's marker stood), rendered as text, first —
 * the leading rule and the sparkle are the second signal, never the only one.
 */
export function AiNote({ text, footer }: { text: string; footer?: ReactNode }) {
  const t = useT();
  return (
    <section aria-label={t.aiNote.verfasst} className="border-l-2 border-ai py-1 pl-4">
      <div className="mb-1.5 flex items-center gap-1.5 text-caption font-semibold text-text-2">
        <Icon name="sparkle" size={13} className="text-ai" />
        {t.aiNote.agent}
      </div>
      <p className="m-0 whitespace-pre-line text-body text-text">{text}</p>
      {footer && <div className="mt-3 border-t border-separator pt-3">{footer}</div>}
    </section>
  );
}
