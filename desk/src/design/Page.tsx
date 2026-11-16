import type { ReactNode } from "react";

/**
 * The one container every view sits in: leading-aligned, the same gutter in
 * every view, so a title's left edge never moves between views. `reading`
 * keeps a document-like view at a readable width; `work` gives a list, a
 * table or a grid the whole window. `cargo xtask check-desk` refuses a view
 * that sets its own width (`max-w-*`, `mx-auto`).
 */
export function Page({
  width,
  title,
  subtitle,
  trailing,
  children,
}: {
  width: "reading" | "work";
  /** The view's title; omitted where the content brings its own `h1`. */
  title?: ReactNode;
  /** One quiet line under the title, never larger than a row. */
  subtitle?: ReactNode;
  /** Beside the title, trailing: a view's one action. */
  trailing?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      data-page={width}
      className={`flex w-full flex-col gap-6 px-(--page-gutter) pt-6 pb-24 lg:px-(--page-gutter-wide) lg:pt-8 ${
        width === "reading" ? "max-w-[calc(var(--page-reading)+2*var(--page-gutter-wide))]" : ""
      }`}
    >
      {title !== undefined && (
        <header className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <h1 className="m-0 text-large-title font-semibold">{title}</h1>
            {subtitle && <p className="measure m-0 mt-1 text-body text-text-2">{subtitle}</p>}
          </div>
          {trailing}
        </header>
      )}
      {children}
    </div>
  );
}
