// The Desk's few icons, drawn on a 20-unit grid with round caps — no icon
// library, because a dozen paths do not justify one.

const PATHS = {
  search: "M8.5 3.5a5 5 0 1 0 0 10 5 5 0 0 0 0-10Zm3.6 8.6 4.4 4.4",
  clock: "M10 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm0 3.5V10l2.5 1.5",
  sparkle: "M10 2.5 11.6 8.4 17.5 10l-5.9 1.6L10 17.5l-1.6-5.9L2.5 10l5.9-1.6Z",
  doc: "M6 2.5h5.5L15 6v11.5H6Zm5.5 0V6H15M8.5 10h4M8.5 13h4",
  tray: "M3 11.5 5 4.5h10l2 7v4H3Zm0 0h4.5l1 2h3l1-2H17",
  flow: "M4.5 5.5h4v4h-4Zm7 5h4v4h-4ZM8.5 7.5h2a1 1 0 0 1 1 1v4",
  grid: "M3.5 3.5h5v5h-5Zm8 0h5v5h-5Zm-8 8h5v5h-5Zm8 0h5v5h-5Z",
  bars: "M4.5 16.5v-5m4 5v-10m4 10v-7m4 7v-12",
  chevron: "M8 5l5 5-5 5",
  copy: "M7 7h9v9H7ZM4 13V4h9",
  check: "M4.5 10.5 8 14l7.5-8",
  thumb:
    "M6.5 9v8h-3V9Zm0 0 3-6c1.4 0 2.2 1 2 2.3L11 8.5h4.3c1 0 1.7.9 1.5 1.9l-1.2 5.3c-.2.8-.9 1.3-1.7 1.3H6.5",
  warn: "M10 3 17.5 16.5h-15Zm0 5v3.5m0 2.5v.5",
  person: "M10 10a3.2 3.2 0 1 0 0-6.4 3.2 3.2 0 0 0 0 6.4Zm-6 6.5c.6-3 3.1-4.5 6-4.5s5.4 1.5 6 4.5",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({
  name,
  size = 16,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
