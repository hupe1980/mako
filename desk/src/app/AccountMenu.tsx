import {
  Button,
  Header,
  Menu,
  MenuItem,
  MenuSection,
  MenuTrigger,
  Popover,
  Separator,
} from "react-aria-components";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { type Katalog, SPRACHEN, type Sprache, useSprache, useT } from "../i18n";
import { monogram } from "../views/kundenSuche";
import { type Appearance, useAppearance } from "./appearance";
import { useIdentity } from "./identity";
import { SHEET } from "./keys";
import { ROLLEN, useRolle } from "./rollen";

const ITEM =
  "flex cursor-default items-center gap-2.5 rounded-[7px] px-2.5 py-1.5 text-body text-text outline-none data-[focused]:bg-accent data-[focused]:text-on-accent";

const APPEARANCES: [Appearance, (k: Katalog["konto"]) => string][] = [
  ["system", (k) => k.system],
  ["light", (k) => k.hell],
  ["dark", (k) => k.dunkel],
];

const SECTION = "px-2.5 pt-1 pb-1 text-caption font-semibold uppercase tracking-wide text-text-2";

/**
 * The person at the foot of the sidebar: who they are, how the Desk looks for
 * them and which Sprache it speaks. The Rolle is not here — it changes what you work on, not who you
 * are, so it stays in the toolbar.
 */
export function AccountMenu({ onShortcuts }: { onShortcuts: () => void }) {
  const identity = useIdentity();
  const { held } = useRolle();
  const [appearance, setAppearance] = useAppearance();
  const [sprache, setSprache] = useSprache();
  const t = useT();
  const name = identity?.name ?? t.konto.konto;

  return (
    <MenuTrigger>
      <Button
        aria-label={t.konto.knopf(name)}
        className="focus-ring flex h-10 w-full items-center gap-2.5 rounded-[8px] px-2 text-left data-[hovered]:bg-fill data-[pressed]:bg-fill-strong"
      >
        <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-fill-strong text-caption font-semibold text-text">
          {identity ? (
            monogram(identity.name.replace(/[-_.]/g, " "))
          ) : (
            <Icon name="person" size={14} />
          )}
        </span>
        <span className="min-w-0 flex-1 truncate text-body font-medium text-text">{name}</span>
        <Icon name="chevron" size={12} className="-rotate-90 text-text-2" />
      </Button>
      <Popover
        placement="top start"
        offset={6}
        className="w-64 rounded-control bg-surface-raised p-1 shadow-popover outline-none"
      >
        <Menu
          aria-label={t.konto.konto}
          onAction={(key) => {
            if (key === "shortcuts") onShortcuts();
            if (key === "logout" && identity?.logout) window.location.assign(identity.logout);
          }}
          className="outline-none"
        >
          <MenuSection>
            <Header className="px-2.5 pt-1.5 pb-2">
              <span className="block text-body font-semibold text-text">{name}</span>
              <span className="block text-caption text-text-2">
                {identity
                  ? held.map((r) => ROLLEN[r].marktrolle).join(" · ")
                  : t.konto.nichtAngemeldet}
              </span>
            </Header>
          </MenuSection>
          <Separator className="my-1 border-t border-separator" />
          <MenuSection
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[appearance]}
            onSelectionChange={(keys) => {
              const [next] = [...keys];
              if (next) setAppearance(next as Appearance);
            }}
          >
            <Header className={SECTION}>{t.konto.erscheinungsbild}</Header>
            {APPEARANCES.map(([id, label]) => (
              <MenuItem key={id} id={id} textValue={label(t.konto)} className={ITEM}>
                {({ isSelected }) => (
                  <>
                    <span className="flex-1">{label(t.konto)}</span>
                    {isSelected && <Icon name="check" size={14} />}
                  </>
                )}
              </MenuItem>
            ))}
          </MenuSection>
          <Separator className="my-1 border-t border-separator" />
          {/* Each language under its own name and in its own lang, so a
              screen reader says „Deutsch“ and „English“ as each is spoken. */}
          <MenuSection
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[sprache]}
            onSelectionChange={(keys) => {
              const [next] = [...keys];
              if (next) setSprache(next as Sprache);
            }}
          >
            <Header className={SECTION}>{t.konto.sprache}</Header>
            {SPRACHEN.map(({ id, name: sprachname }) => (
              <MenuItem key={id} id={id} textValue={sprachname} lang={id} className={ITEM}>
                {({ isSelected }) => (
                  <>
                    <span className="flex-1">{sprachname}</span>
                    {isSelected && <Icon name="check" size={14} />}
                  </>
                )}
              </MenuItem>
            ))}
          </MenuSection>
          <Separator className="my-1 border-t border-separator" />
          <MenuItem id="shortcuts" textValue={t.konto.tastaturkuerzel} className={ITEM}>
            <span className="flex-1">{t.konto.tastaturkuerzel}</span>
            <Kbd>{SHEET.keys}</Kbd>
          </MenuItem>
          {identity?.logout && (
            <MenuItem id="logout" textValue={t.konto.abmelden} className={ITEM}>
              {t.konto.abmelden}
            </MenuItem>
          )}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}
