import type { ReactNode } from "react";

/** A content surface. */
export function Panel({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <section
      className={`squircle rounded-panel border border-separator bg-surface shadow-[0_1px_2px_rgb(0_0_0/0.04)] ${className}`}
    >
      {children}
    </section>
  );
}

/** A quiet, centred state: nothing to show, nothing went wrong. */
export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-16 text-center">
      <p className="m-0 text-title font-semibold text-text">{title}</p>
      {children && <div className="max-w-sm text-callout text-text-2">{children}</div>}
    </div>
  );
}

/** A state that needs the reader: a refusal or a failure, with the service's words —
 * or an assurance (`info`) the reader can rely on. */
export function Notice({
  tone,
  title,
  children,
}: {
  tone: "refusal" | "failure" | "info";
  title: string;
  children?: ReactNode;
}) {
  const look =
    tone === "failure"
      ? "border-critical/30 bg-critical-fill"
      : tone === "info"
        ? "border-done/30 bg-done-fill"
        : "border-separator bg-fill";
  return (
    <div role="status" className={`squircle rounded-panel border p-4 ${look}`}>
      <p className="m-0 text-callout font-semibold text-text">{title}</p>
      {children && <div className="mt-1 text-body text-text-2">{children}</div>}
    </div>
  );
}
