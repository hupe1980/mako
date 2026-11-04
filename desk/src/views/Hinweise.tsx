import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button as AriaButton, Link } from "react-aria-components";
import { useRolle } from "../app/rollen";
import { type Hinweis, hinweise } from "../data/agent";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import type { Instant } from "../format/brands";
import { formatCount } from "../format/count";
import { Bewertung } from "./Bewertung";

const STORE = "mako-desk.ausgeblendet";

/** Skills a person silenced — in this browser only; it hides, it deletes nothing. */
function useAusgeblendet(): [string[], (skill: string, aus: boolean) => void] {
  const [skills, setSkills] = useState<string[]>(() => {
    try {
      return JSON.parse(localStorage.getItem(STORE) ?? "[]") as string[];
    } catch {
      return [];
    }
  });
  const set = (skill: string, aus: boolean) => {
    const next = aus ? [...new Set([...skills, skill])] : skills.filter((s) => s !== skill);
    setSkills(next);
    try {
      localStorage.setItem(STORE, JSON.stringify(next));
    } catch {
      // storage blocked: the choice lasts for this tab
    }
  };
  return [skills, set];
}

const ANKER_ART: Record<string, string> = { malo: "MaLo", melo: "MeLo", process: "Prozess" };

function Zeile({ h, onAusblenden }: { h: Hinweis; onAusblenden: (() => void) | undefined }) {
  const dringend = h.prioritaet === "urgent";
  return (
    <li className="flex gap-3 border-b border-ai-border py-3 last:border-b-0">
      <span
        aria-hidden="true"
        className={`mt-1.5 size-2 shrink-0 rounded-full ${dringend ? "bg-critical" : "bg-ai"}`}
      />
      <div className="min-w-0 flex-1">
        <p className="m-0 flex flex-wrap items-center gap-x-2 text-caption font-semibold text-ai">
          {h.titel ?? h.skill ?? "Assistent"}
          {dringend && <span className="text-critical">dringend</span>}
        </p>
        <p className="m-0 mt-0.5 text-callout text-text italic">{h.text}</p>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {h.anker.map((a) =>
            a.art === "process" ? (
              <span key={a.wert} className="font-mono text-caption text-text-2">
                Prozess {a.wert}
              </span>
            ) : (
              <Link
                key={a.wert}
                href={`/prozess?key=${a.wert}`}
                className="focus-ring inline-flex h-6 items-center gap-1 rounded-full border border-ai-border bg-surface px-2 text-caption text-text no-underline data-[hovered]:bg-fill"
              >
                {ANKER_ART[a.art] ?? a.art} <span className="font-mono">{a.wert}</span>
              </Link>
            ),
          )}
          {h.faellig && (
            // The Hinweis's own due date — by when a person should act on it,
            // not the Frist of the process it is about.
            <span className="inline-flex items-center gap-1.5 text-caption text-text-2">
              bearbeiten <FristUhr due={h.faellig as Instant} />
            </span>
          )}
          {onAusblenden && h.skill && (
            <AriaButton
              onPress={onAusblenden}
              className="focus-ring ml-auto h-6 rounded-full px-2 text-caption text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
            >
              {h.titel ?? h.skill} ausblenden
            </AriaButton>
          )}
        </div>
        {h.run && (
          <div className="mt-2.5">
            <Bewertung run={h.run} />
          </div>
        )}
      </div>
    </li>
  );
}

/**
 * Hinweise: what the assistant's skills noticed on their own runs and left
 * for a person — each filed under its Anker, written in the row's own words,
 * a skill silenced in one click and shown again in one.
 */
export function Hinweise({ nurFuer }: { nurFuer?: string[] }) {
  const { rolle } = useRolle();
  const [ausgeblendet, setAusgeblendet] = useAusgeblendet();
  const [alle, setAlle] = useState(false);
  const rows = useQuery({
    queryKey: [rolle, "agentd", "hinweise"],
    queryFn: hinweise,
    refetchInterval: 60_000,
    retry: false,
  });
  const passend = (rows.data ?? []).filter(
    (h) => !nurFuer || h.anker.some((a) => nurFuer.includes(a.wert)),
  );
  const sichtbar = passend.filter((h) => alle || !h.skill || !ausgeblendet.includes(h.skill));
  const versteckt = passend.length - sichtbar.length;
  const titelVon = new Map(
    (rows.data ?? []).flatMap((h) => (h.skill && h.titel ? [[h.skill, h.titel] as const] : [])),
  );
  // An unreachable assistant hides this section: the rest of the Desk works.
  if (!rows.data || passend.length === 0) return null;

  return (
    <section
      aria-labelledby="hinweise"
      className="squircle rounded-panel border border-ai-border bg-ai-fill px-5 py-3"
    >
      <h2
        id="hinweise"
        className="m-0 flex items-center gap-1.5 py-1 text-callout font-semibold text-ai"
      >
        <Icon name="sparkle" size={14} />
        Hinweise des Assistenten
        <span className="font-normal text-text-2">{formatCount(sichtbar.length)}</span>
      </h2>
      <ul className="m-0 list-none p-0">
        {sichtbar.map((h) => (
          <Zeile
            key={h.task}
            h={h}
            onAusblenden={h.skill ? () => setAusgeblendet(h.skill as string, true) : undefined}
          />
        ))}
      </ul>
      {versteckt > 0 && (
        <AriaButton
          onPress={() => setAlle(!alle)}
          className="focus-ring mt-1 mb-1 rounded-full text-caption text-text-2 data-[hovered]:text-text"
        >
          {alle
            ? "Ausgeblendete wieder verbergen"
            : `${formatCount(versteckt)} ausgeblendet · zeigen`}
        </AriaButton>
      )}
      {alle && ausgeblendet.length > 0 && (
        <p className="m-0 mb-2 flex flex-wrap gap-2 text-caption text-text-2">
          {ausgeblendet.map((s) => (
            <AriaButton
              key={s}
              onPress={() => setAusgeblendet(s, false)}
              className="focus-ring rounded-full border border-ai-border px-2 data-[hovered]:bg-fill"
            >
              {titelVon.get(s) ?? "Fähigkeit"} wieder einblenden
            </AriaButton>
          ))}
        </p>
      )}
    </section>
  );
}
