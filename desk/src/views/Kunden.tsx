import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Link } from "react-aria-components";
import { nichtErreichbar } from "../app/dienste";
import { ServiceUnavailable } from "../data/mcp";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SearchField } from "../design/SearchField";
import { monogram, useKundenSuche } from "./kundenSuche";

/** Kunden: the Lieferant's starting point — find a customer by what you remember. */
export function KundenView() {
  const { q } = useSearch({ from: "/kunden" });
  const navigate = useNavigate({ from: "/kunden" });
  const [typed, setTyped] = useState(q ?? "");
  useEffect(() => {
    navigate({ search: typed.trim() ? { q: typed.trim() } : {}, replace: true });
  }, [typed, navigate]);
  const search = useKundenSuche(typed);
  const short = typed.trim().length < 3;

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Kunden</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Einen Kunden über Namen, Kundennummer oder die MaLo-ID einer Lieferstelle finden.
        </p>
      </header>

      <SearchField
        aria-label="Kunde"
        placeholder="Name, Kundennummer oder MaLo-ID"
        value={typed}
        onChange={setTyped}
        autoFocus
      />

      {short && (
        <EmptyState title="Wen suchen Sie?">
          Drei Zeichen genügen — oder von überall <Kbd>⌘K</Kbd>.
        </EmptyState>
      )}
      {!short && search.isError && (
        <Notice
          tone="failure"
          title={
            search.error instanceof ServiceUnavailable
              ? nichtErreichbar(search.error.service)
              : "Die Suche ist fehlgeschlagen"
          }
        >
          {search.error.message}
        </Notice>
      )}
      {!short && search.data && search.data.treffer.length === 0 && (
        <EmptyState title="Kein Kunde gefunden.">
          Für „{search.data.query}“ gibt es keinen Namen, keine Kundennummer und keine MaLo-ID.
        </EmptyState>
      )}
      {!short && search.data && search.data.treffer.length > 0 && (
        <Panel className="overflow-hidden">
          <ul className="m-0 list-none p-0">
            {search.data.treffer.map((k) => (
              <li key={k.id} className="border-b border-separator last:border-b-0">
                <Link
                  href={`/kunde/${k.id}`}
                  className="focus-ring flex items-center gap-4 px-5 py-3 no-underline data-[hovered]:bg-fill"
                >
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-body font-semibold text-accent">
                    {monogram(k.name)}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-callout font-medium text-text">
                      {k.name ?? "Ohne Namen"}
                    </span>
                    <span className="block text-body text-text-2">
                      {k.kunden_nr ? (
                        <span className="font-mono">{k.kunden_nr}</span>
                      ) : (
                        "ohne Nummer"
                      )}
                      {" · "}
                      {k.lieferstellen === 1
                        ? "1 Lieferstelle"
                        : `${k.lieferstellen} Lieferstellen`}
                    </span>
                  </span>
                  <Icon name="chevron" size={16} className="text-text-2" />
                </Link>
              </li>
            ))}
          </ul>
          {search.data.mehr && (
            <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
              Mehr als 20 Treffer — weiter tippen, um einzugrenzen.
            </p>
          )}
        </Panel>
      )}
    </div>
  );
}
