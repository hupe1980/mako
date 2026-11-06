import { useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import {
  Autocomplete,
  Dialog,
  Header,
  Input,
  Menu,
  MenuItem,
  MenuSection,
  Modal,
  ModalOverlay,
  SearchField,
  useFilter,
} from "react-aria-components";
import { Icon, type IconName } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { monogram, useKundenSuche } from "../views/kundenSuche";
import { frageMoeglich, frageStellen } from "./frage";
import { PALETTE } from "./keys";
import { ROLLEN, type Rolle, useRolle } from "./rollen";

interface Command {
  id: string;
  label: string;
  icon: IconName;
  hint?: string;
  run: () => void;
  /** Found by a service, so the palette's own text filter must not hide it. */
  direct?: boolean;
  /** Initials shown instead of the icon — a customer, not a command. */
  monogram?: string;
}

/** What a typed key most likely is — a label only; nothing is validated here. */
export function keyKind(key: string): string {
  if (/^\d{11}$/.test(key)) return "MaLo-ID";
  if (/^DE[0-9A-Z]{31}$/i.test(key)) return "MeLo-ID";
  return "Vorgangsnummer";
}

const ITEM =
  "flex cursor-default items-center gap-3 rounded-[8px] px-3 py-2 text-callout text-text outline-none data-[focused]:bg-accent data-[focused]:text-on-accent";

/**
 * ⌘K: every view and every action, by name. Typing a business key offers to
 * open its process straight away.
 */
export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [input, setInput] = useState("");
  const navigate = useNavigate();
  const { rolle, held, wear } = useRolle();
  const { contains } = useFilter({ sensitivity: "base" });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (PALETTE.match?.(e)) {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  const close = () => {
    onOpenChange(false);
    setInput("");
  };

  const typed = input.trim();
  // A business key — MaLo-ID, MeLo-ID, Vorgangsnummer — always has a digit;
  // a name never needs the „Prozess … öffnen" row.
  const direct: Command[] =
    typed.length >= 5 && /^[0-9A-Za-z-]+$/.test(typed) && /\d/.test(typed)
      ? [
          {
            id: "open-key",
            label: `Prozess ${typed} öffnen`,
            icon: "flow",
            hint: keyKind(typed),
            run: () => navigate({ to: "/prozess", search: { key: typed } }),
          },
        ]
      : [];

  // The Lieferant's Anker: customers, searched in vertragd as the person types.
  const suche = useKundenSuche(open ? typed : "");
  const kunden: Command[] =
    rolle === "lieferant" && typed.length >= 3
      ? (suche.data?.treffer ?? []).map((k) => ({
          id: `kunde-${k.id}`,
          label: k.name ?? "Ohne Namen",
          icon: "person" as const,
          monogram: monogram(k.name),
          hint: [
            k.kunden_nr,
            k.lieferstellen === 1 ? "1 Lieferstelle" : `${k.lieferstellen} Lieferstellen`,
          ]
            .filter(Boolean)
            .join(" · "),
          direct: true,
          run: () => navigate({ to: "/kunde/$id", params: { id: k.id } }),
        }))
      : [];

  // On a page with a Fragen panel, anything typed can go to the assistant.
  const fragen: Command[] =
    open && typed.length >= 3 && frageMoeglich()
      ? [
          {
            id: "fragen",
            label: `„${typed}"`,
            icon: "sparkle",
            hint: "Den Assistenten fragen",
            direct: true,
            run: () => frageStellen(typed),
          },
        ]
      : [];

  const views: Command[] = [
    {
      id: "ueberblick",
      label: "Überblick",
      icon: "bars",
      run: () => navigate({ to: "/ueberblick", search: {} }),
    },
    {
      id: "assistent",
      label: "Assistent",
      icon: "sparkle",
      run: () => navigate({ to: "/assistent", search: {} }),
    },
    {
      id: "fristen",
      label: "Fristen überschritten",
      icon: "clock",
      run: () => navigate({ to: "/fristen", search: {} }),
    },
    ...(rolle === "lieferant"
      ? [
          {
            id: "kunden",
            label: "Kunden",
            icon: "person" as const,
            run: () => navigate({ to: "/kunden", search: {} }),
          },
        ]
      : []),
    {
      id: "prozesse",
      label: "Prozesse",
      icon: "flow",
      run: () => navigate({ to: "/prozess", search: {} }),
    },
    ...(import.meta.env.DEV
      ? [
          {
            id: "gallery",
            label: "Gallery",
            icon: "grid" as const,
            hint: "Entwicklung",
            run: () => navigate({ to: "/dev/gallery" }),
          },
        ]
      : []),
  ];

  const rollen: Command[] = held
    .filter((r) => r !== rolle)
    .map((r: Rolle) => ({
      id: `rolle-${r}`,
      label: `Rolle: ${ROLLEN[r].label}`,
      icon: "tray",
      hint: `⌃${ROLLEN[r].key}`,
      run: () => wear(r),
    }));

  const sections: [string, Command[]][] = (
    [
      ["Kunden", kunden],
      ["Öffnen", direct],
      ["Assistent", fragen],
      ["Ansichten", views],
      ["Rolle wechseln", rollen],
    ] as [string, Command[]][]
  ).filter(([, items]) => items.length > 0);

  const all = sections.flatMap(([, items]) => items);

  return (
    <ModalOverlay
      isOpen={open}
      onOpenChange={(o) => (o ? onOpenChange(true) : close())}
      isDismissable
      className="fixed inset-0 z-50 bg-black/20 data-[entering]:animate-in"
    >
      <Modal className="mx-auto mt-[14vh] w-[min(600px,calc(100vw-32px))] outline-none">
        <Dialog
          aria-label="Befehle"
          className="squircle overflow-hidden rounded-panel bg-surface-raised shadow-popover outline-none"
        >
          <Autocomplete
            inputValue={input}
            onInputChange={setInput}
            filter={(text, query) => text === "__direct__" || contains(text, query)}
          >
            <SearchField
              aria-label="Befehl oder Geschäftsschlüssel"
              autoFocus
              className="border-b border-separator"
            >
              <div className="flex items-center gap-3 px-4">
                <Icon name="search" size={18} className="text-text-2" />
                <Input
                  placeholder="Befehl oder Geschäftsschlüssel …"
                  className="h-14 flex-1 bg-transparent text-[17px] text-text outline-none placeholder:text-text-2 [&::-webkit-search-cancel-button]:hidden"
                />
                <Kbd>esc</Kbd>
              </div>
            </SearchField>
            <Menu
              aria-label="Befehle"
              onAction={(id) => {
                all.find((c) => c.id === id)?.run();
                close();
              }}
              renderEmptyState={() => (
                <p className="m-0 px-4 py-6 text-center text-body text-text-2">
                  Kein Befehl passt zu „{input}“.
                </p>
              )}
              className="max-h-[50vh] overflow-auto p-2 outline-none"
            >
              {sections.map(([title, items]) => (
                <MenuSection key={title} className="mb-1">
                  <Header className="px-3 pb-1 pt-2 text-caption font-semibold uppercase tracking-wide text-text-2">
                    {title}
                  </Header>
                  {items.map((c) => (
                    <MenuItem
                      key={c.id}
                      id={c.id}
                      textValue={c.id === "open-key" || c.direct ? "__direct__" : c.label}
                      className={ITEM}
                    >
                      {c.monogram ? (
                        <span className="flex size-6 items-center justify-center rounded-full bg-[color-mix(in_srgb,currentColor_14%,transparent)] text-caption font-semibold">
                          {c.monogram}
                        </span>
                      ) : (
                        <Icon name={c.icon} size={16} />
                      )}
                      <span className="flex-1">{c.label}</span>
                      {c.hint && <span className="text-body opacity-70">{c.hint}</span>}
                    </MenuItem>
                  ))}
                </MenuSection>
              ))}
            </Menu>
          </Autocomplete>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
