import { Button, Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import { Kbd } from "../design/Kbd";
import { useT } from "../i18n";
import { ROLLEN, type Rolle, useRolle } from "./rollen";

function Dot({ rolle }: { rolle: Rolle }) {
  return <span data-rolle={rolle} className="size-2 rounded-full bg-accent" aria-hidden="true" />;
}

/**
 * The Rolle being worn — the toolbar's leading item on every screen. With one
 * Rolle it is a label; with several, a menu (⌃1–⌃4).
 */
export function RolleBadge() {
  const t = useT();
  const { rolle, held, wear } = useRolle();
  const label = ROLLEN[rolle].label;
  const look =
    "inline-flex h-7 items-center gap-2 rounded-full bg-[color-mix(in_srgb,var(--accent)_12%,transparent)] px-3 text-body font-semibold text-accent";

  if (held.length < 2) {
    return (
      <span className={look}>
        <Dot rolle={rolle} />
        {label}
      </span>
    );
  }
  return (
    <MenuTrigger>
      <Button aria-label={t.rolleBadge.knopf(label)} className={`focus-ring ${look}`}>
        <Dot rolle={rolle} />
        {label}
        <span aria-hidden="true" className="text-caption opacity-70">
          ▾
        </span>
      </Button>
      <Popover
        offset={6}
        className="min-w-56 rounded-control bg-surface-raised p-1 shadow-popover outline-none"
      >
        <Menu
          aria-label={t.rolleBadge.wechseln}
          selectionMode="single"
          selectedKeys={[rolle]}
          onAction={(key) => wear(key as Rolle)}
          className="outline-none"
        >
          {held.map((r) => (
            <MenuItem
              key={r}
              id={r}
              textValue={ROLLEN[r].label}
              className="flex cursor-default items-center gap-2 rounded-[7px] px-2.5 py-1.5 text-body text-text outline-none data-[focused]:bg-accent data-[focused]:text-on-accent"
            >
              <Dot rolle={r} />
              <span className="flex-1">
                {ROLLEN[r].label}
                <span className="ml-1.5 opacity-60">{ROLLEN[r].marktrolle}</span>
              </span>
              <Kbd>⌃{ROLLEN[r].key}</Kbd>
            </MenuItem>
          ))}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}
