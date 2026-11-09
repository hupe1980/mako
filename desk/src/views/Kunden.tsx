import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Link } from "react-aria-components";
import { nichtErreichbar } from "../app/dienste";
import { ServiceUnavailable } from "../data/mcp";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SearchField } from "../design/SearchField";
import { useT } from "../i18n";
import { monogram, useKundenSuche } from "./kundenSuche";

/** Kunden: the Lieferant's starting point — find a customer by what you remember. */
export function KundenView() {
  const t = useT();
  const k = t.kundenAnsicht;
  const { q } = useSearch({ from: "/kunden" });
  const navigate = useNavigate({ from: "/kunden" });
  const [typed, setTyped] = useState(q ?? "");
  useEffect(() => {
    navigate({ search: typed.trim() ? { q: typed.trim() } : {}, replace: true });
  }, [typed, navigate]);
  const search = useKundenSuche(typed);
  const short = typed.trim().length < 3;

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-4 py-6 md:px-8 md:py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">{k.titel}</h1>
        <p className="m-0 mt-1 text-callout text-text-2">{k.untertitel}</p>
      </header>

      <SearchField
        aria-label={k.feld}
        placeholder={k.platzhalter}
        value={typed}
        onChange={setTyped}
        autoFocus
      />

      {short && (
        <EmptyState title={k.wen}>
          {k.dreiZeichen} <Kbd>⌘K</Kbd>.
        </EmptyState>
      )}
      {!short && search.isError && (
        <Notice
          tone="failure"
          title={
            search.error instanceof ServiceUnavailable
              ? nichtErreichbar(search.error.service)
              : k.fehler
          }
        >
          {search.error.message}
        </Notice>
      )}
      {!short && search.data && search.data.treffer.length === 0 && (
        <EmptyState title={k.keiner}>{k.keinerText(search.data.query)}</EmptyState>
      )}
      {!short && search.data && search.data.treffer.length > 0 && (
        <Panel className="overflow-hidden">
          <ul className="m-0 list-none p-0">
            {search.data.treffer.map((treffer) => (
              <li key={treffer.id} className="border-b border-separator last:border-b-0">
                <Link
                  href={`/kunde/${treffer.id}`}
                  className="focus-ring flex items-center gap-4 px-5 py-3 no-underline data-[hovered]:bg-fill"
                >
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-body font-semibold text-accent">
                    {monogram(treffer.name)}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-callout font-medium text-text">
                      {treffer.name ?? k.ohneNamen}
                    </span>
                    <span className="block text-body text-text-2">
                      {treffer.kunden_nr ? (
                        <span className="font-mono">{treffer.kunden_nr}</span>
                      ) : (
                        k.ohneNummer
                      )}
                      {" · "}
                      {k.lieferstellen(treffer.lieferstellen, `${treffer.lieferstellen}`)}
                    </span>
                  </span>
                  <Icon name="chevron" size={16} className="text-text-2" />
                </Link>
              </li>
            ))}
          </ul>
          {search.data.mehr && (
            <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
              {k.mehr}
            </p>
          )}
        </Panel>
      )}
    </div>
  );
}
