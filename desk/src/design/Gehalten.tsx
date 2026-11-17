import { UNDO } from "../app/keys";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/** A held decision, until it is sent or taken back. */
export function Gehalten({ text }: { text: string }) {
  const t = useT();
  return (
    <p role="status" className="m-0 flex items-center gap-2 text-body text-text">
      <Icon name="check" size={14} className="text-accent" />
      {t.entscheidung.gehalten(text, UNDO.keys)}
    </p>
  );
}
