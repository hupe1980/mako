import { Outlet, useRouter } from "@tanstack/react-router";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button, Link, RouterProvider } from "react-aria-components";
import { Icon, type IconName } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { formatCount } from "../format/count";
import { useFreigabenZahl } from "../views/Freigaben";
import { AccountMenu } from "./AccountMenu";
import { CommandPalette } from "./CommandPalette";
import { HoldProvider } from "./Holds";
import { SHEET } from "./keys";
import { RolleBadge } from "./RolleBadge";
import { ROLLEN, useRolle } from "./rollen";
import { ShortcutSheet } from "./ShortcutSheet";

function NavItem({
  to,
  match = to,
  icon,
  children,
}: {
  to: string;
  /** The path prefix that marks this item current, when wider than `to`. */
  match?: string;
  icon: IconName;
  children: ReactNode;
}) {
  const router = useRouter();
  const current = router.state.location.pathname.startsWith(match);
  return (
    <Link
      href={to}
      {...(current ? { "aria-current": "page" as const } : {})}
      className={`focus-ring flex h-8 items-center gap-2.5 rounded-[8px] px-2.5 text-body font-medium no-underline ${
        current
          ? "bg-fill-strong text-text"
          : "text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
      }`}
    >
      <Icon name={icon} size={16} className={current ? "text-accent" : ""} />
      {children}
    </Link>
  );
}

/** How many decisions wait for a person — shown only when there are some. */
function FreigabenZahl() {
  const n = useFreigabenZahl();
  if (!n) return null;
  return (
    <span className="ml-auto rounded-full bg-accent px-1.5 text-caption font-semibold text-on-accent tabular-nums">
      {formatCount(n)}
    </span>
  );
}

/** Sidebar, toolbar and the view: the frame every screen of the Desk sits in. */
export function Shell() {
  const router = useRouter();
  const [palette, setPalette] = useState(false);
  const [sheet, setSheet] = useState(false);
  const { rolle } = useRolle();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (SHEET.match?.(e)) {
        e.preventDefault();
        setSheet((open) => !open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A Rolle opens on its own home: after a switch, no page of the Rolle left
  // stays on screen.
  const worn = useRef(rolle);
  useEffect(() => {
    if (worn.current === rolle) return;
    worn.current = rolle;
    router.navigate({ to: ROLLEN[rolle].home, search: {} });
  }, [rolle, router]);
  return (
    <RouterProvider
      navigate={(to) => router.navigate({ to })}
      useHref={(to) => router.buildLocation({ to }).href}
    >
      <HoldProvider>
        <div className="grid h-full grid-cols-[232px_1fr]">
          <aside className="flex flex-col gap-1 border-r border-separator bg-sidebar px-3 pt-4 pb-3">
            <div className="mb-4 flex items-baseline gap-1 px-2.5">
              <span className="text-callout font-bold tracking-tight">mako</span>
              <span className="text-callout text-text-2">Desk</span>
            </div>
            <nav aria-label="Ansichten" className="flex flex-col gap-0.5">
              <NavItem to="/ueberblick" icon="bars">
                Überblick
              </NavItem>
              <NavItem to="/freigaben" icon="check">
                Freigaben
                <FreigabenZahl />
              </NavItem>
              {rolle === "lieferant" && (
                <NavItem to="/heute" icon="tray">
                  Heute
                </NavItem>
              )}
              {rolle === "lieferant" && (
                <NavItem to="/kunden" match="/kunde" icon="person">
                  Kunden
                </NavItem>
              )}
              <NavItem to="/prozess" icon="flow">
                Prozesse
              </NavItem>
              <NavItem to="/assistent" icon="sparkle">
                Assistent
              </NavItem>
            </nav>
            <div className="flex-1" />
            {import.meta.env.DEV && (
              <NavItem to="/dev/gallery" icon="grid">
                Gallery
              </NavItem>
            )}
            <div className="mt-1 border-t border-separator pt-2">
              <AccountMenu onShortcuts={() => setSheet(true)} />
            </div>
          </aside>

          <main className="relative min-w-0 overflow-auto">
            <header className="material sticky top-0 z-10 flex h-13 items-center gap-3 border-b border-separator px-5">
              <RolleBadge />
              <div className="flex-1" />
              <Button
                onPress={() => setPalette(true)}
                className="focus-ring flex h-7 w-64 items-center gap-2 rounded-control bg-fill px-2.5 text-body text-text-2 data-[hovered]:bg-fill-strong"
              >
                <Icon name="search" size={14} />
                <span className="flex-1 text-left">Suchen oder Befehl</span>
                <Kbd>⌘K</Kbd>
              </Button>
            </header>
            <Outlet />
          </main>
        </div>
      </HoldProvider>
      <CommandPalette open={palette} onOpenChange={setPalette} />
      <ShortcutSheet open={sheet} onOpenChange={setSheet} />
    </RouterProvider>
  );
}
