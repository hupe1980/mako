import type { ReactNode } from "react";

/** A key as printed on the keyboard: `⌘K`, `J`, `Space`. */
export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-[5px] border border-separator bg-surface px-1 font-sans text-caption font-medium text-text-2 shadow-[0_1px_0_var(--separator)]">
      {children}
    </kbd>
  );
}
