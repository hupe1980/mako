import { Outlet, useRouter, useRouterState } from "@tanstack/react-router";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button, Dialog, Link, Modal, ModalOverlay, RouterProvider } from "react-aria-components";
import { Icon, type IconName } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { formatCount } from "../format/count";
import { useT } from "../i18n";
import { useFreigabenZahl } from "../views/Freigaben";
import { AccountMenu } from "./AccountMenu";
import { CommandPalette } from "./CommandPalette";
import { HoldProvider } from "./Holds";
import { SHEET } from "./keys";
import { LaufPills } from "./LaufPills";
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
  // Subscribed, so the mark follows a client-side navigation.
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const current = pathname.startsWith(match);
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

/** The views, the gallery in dev and the account: one sidebar, docked or as a sheet. */
function Seitenleiste({ onShortcuts }: { onShortcuts: () => void }) {
  const { rolle } = useRolle();
  const t = useT();
  return (
    <>
      <div className="mb-4 flex items-baseline gap-1 px-2.5">
        <span className="text-callout font-bold tracking-tight">mako</span>
        <span className="text-callout text-text-2">Desk</span>
      </div>
      <nav aria-label={t.shell.ansichten} className="flex flex-col gap-0.5">
        <NavItem to="/heute" icon="tray">
          {t.shell.heute}
        </NavItem>
        <NavItem to="/ueberblick" icon="bars">
          {t.shell.ueberblick}
        </NavItem>
        <NavItem to="/freigaben" icon="check">
          {t.shell.freigaben}
          <FreigabenZahl />
        </NavItem>
        {rolle === "lieferant" && (
          <NavItem to="/kunden" match="/kunde" icon="person">
            {t.shell.kunden}
          </NavItem>
        )}
        <NavItem to="/prozess" icon="flow">
          {t.shell.prozesse}
        </NavItem>
        <NavItem to="/assistent" icon="sparkle">
          {t.shell.assistent}
        </NavItem>
        <NavItem to="/einrichtung" icon="tune">
          {t.einrichtung.nav}
        </NavItem>
      </nav>
      <div className="flex-1" />
      {import.meta.env.DEV && (
        <NavItem to="/dev/gallery" icon="grid">
          {t.shell.gallery}
        </NavItem>
      )}
      <div className="mt-1 border-t border-separator pt-2">
        <AccountMenu onShortcuts={onShortcuts} />
      </div>
    </>
  );
}

/** Sidebar, toolbar and the view: the frame every screen of the Desk sits in. */
export function Shell() {
  const router = useRouter();
  const [palette, setPalette] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [menue, setMenue] = useState(false);
  const { rolle } = useRolle();
  const t = useT();

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

  // A choice in the sheet navigates; the sheet closes behind it.
  const pathname = useRouterState({ select: (st) => st.location.pathname });
  // biome-ignore lint/correctness/useExhaustiveDependencies: closes on every navigation
  useEffect(() => setMenue(false), [pathname]);

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
        <div className="grid h-full md:grid-cols-[var(--pane-sidebar)_1fr]">
          <aside className="hidden flex-col gap-1 border-r border-separator bg-sidebar px-3 pt-4 pb-3 md:flex">
            <Seitenleiste onShortcuts={() => setSheet(true)} />
          </aside>
          {/* At a phone's width the sidebar is a sheet: the same views, opened
              from the top bar, closed by esc, a tap outside or a choice. */}
          <ModalOverlay
            isOpen={menue}
            onOpenChange={setMenue}
            isDismissable
            className="fixed inset-0 z-30 bg-black/30 md:hidden"
          >
            <Modal className="schublade fixed inset-y-0 left-0 flex w-[min(var(--pane-sidebar),calc(100vw-48px))] outline-none">
              <Dialog
                aria-label={t.shell.ansichten}
                className="flex w-full flex-col gap-1 overflow-auto bg-sidebar px-3 pt-4 pb-3 shadow-popover outline-none"
              >
                <Seitenleiste
                  onShortcuts={() => {
                    setMenue(false);
                    setSheet(true);
                  }}
                />
              </Dialog>
            </Modal>
          </ModalOverlay>

          {/* A scroll into view — a focused citation, a hash link, focus moving
              down a list — stops below the sticky bar, not under it. */}
          <main className="relative min-w-0 scroll-pt-(--topbar) overflow-auto">
            <header className="material sticky top-0 z-10 flex h-(--topbar) items-center gap-3 border-b border-separator px-4 md:px-5">
              <Button
                aria-label={t.shell.menue}
                onPress={() => setMenue(true)}
                className="focus-ring -ml-1 flex size-8 shrink-0 items-center justify-center rounded-control text-text-2 data-[hovered]:bg-fill md:hidden"
              >
                <Icon name="menu" size={18} />
              </Button>
              <RolleBadge />
              <LaufPills />
              <div className="flex-1" />
              <Button
                onPress={() => setPalette(true)}
                className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center gap-2 rounded-control bg-fill text-body text-text-2 data-[hovered]:bg-fill-strong md:h-7 md:w-64 md:justify-start md:px-2.5"
              >
                <Icon name="search" size={14} />
                {/* The name stays the visible text; at a phone's width only the icon shows. */}
                <span className="sr-only md:not-sr-only md:flex-1 md:text-left">
                  {t.shell.suchen}
                </span>
                <span className="hidden md:contents">
                  <Kbd>⌘K</Kbd>
                </span>
              </Button>
            </header>
            <Outlet />
          </main>
        </div>
        {/* Inside the holds: a decision made in ⌘K is held like any other. */}
        <CommandPalette open={palette} onOpenChange={setPalette} />
      </HoldProvider>
      <ShortcutSheet open={sheet} onOpenChange={setSheet} />
    </RouterProvider>
  );
}
