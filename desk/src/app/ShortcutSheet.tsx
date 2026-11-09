import { Dialog, Heading, Modal, ModalOverlay } from "react-aria-components";
import { Kbd } from "../design/Kbd";
import { useT } from "../i18n";
import { SHORTCUTS } from "./keys";

/** ⌘/: every key the Desk answers to, printed from the table that handles them. */
export function ShortcutSheet({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const groups = [...new Set(SHORTCUTS.map((s) => s.group))];
  return (
    <ModalOverlay
      isOpen={open}
      onOpenChange={onOpenChange}
      isDismissable
      className="fixed inset-0 z-50 bg-black/20"
    >
      <Modal className="mx-auto mt-[16vh] w-[min(420px,calc(100vw-32px))] outline-none">
        <Dialog className="squircle rounded-panel bg-surface-raised p-5 shadow-popover outline-none">
          <Heading slot="title" className="m-0 mb-3 text-title font-bold tracking-tight">
            {t.tasten.sheet}
          </Heading>
          {groups.map((g) => (
            <section key={g} className="mb-3 last:mb-0">
              <h3 className="m-0 mb-1 text-caption font-semibold uppercase tracking-wide text-text-2">
                {g}
              </h3>
              <dl className="m-0">
                {SHORTCUTS.filter((s) => s.group === g).map((s) => (
                  <div
                    key={s.keys}
                    className="flex items-center justify-between border-b border-separator py-1.5 last:border-b-0"
                  >
                    <dt className="text-body text-text">{s.label}</dt>
                    <dd className="m-0">
                      <Kbd>{s.keys}</Kbd>
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
