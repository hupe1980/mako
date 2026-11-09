import type { ReactNode } from "react";
import {
  Table as AriaTable,
  Cell,
  Column,
  Row,
  TableBody,
  TableHeader,
} from "react-aria-components";

/** One column: its header, how a row fills it, and whether it names the row. */
export interface Spalte<T> {
  id: string;
  titel: string;
  zelle: (row: T) => ReactNode;
  /** The column a screen reader announces as the row's name — exactly one. */
  rowHeader?: boolean;
  /** Width and alignment, as Tailwind classes. */
  className?: string;
}

/**
 * A table of a service's page, in the order the service returned it — it
 * offers no sorting, because the Desk never orders a page as if it were the
 * whole list. React Aria's grid: ↑ ↓ ← → move cell by cell, and each row is
 * announced with its row-header cell.
 */
export function Table<T>({
  label,
  spalten,
  zeilen,
  zeilenId,
  leer,
}: {
  label: string;
  spalten: Spalte<T>[];
  zeilen: T[];
  /** A stable id per row; a page replaced whole on each read may use its index. */
  zeilenId: (row: T, index: number) => string;
  leer?: ReactNode;
}) {
  const indexed = zeilen.map((row, i) => ({ id: zeilenId(row, i), row }));
  return (
    <AriaTable aria-label={label} className="w-full border-collapse text-body">
      <TableHeader className="border-b border-separator">
        {spalten.map((s) => (
          <Column
            key={s.id}
            id={s.id}
            {...(s.rowHeader ? { isRowHeader: true } : {})}
            className={`px-5 py-2 text-left text-caption font-semibold uppercase tracking-wide text-text-2 ${s.className ?? ""}`}
          >
            {s.titel}
          </Column>
        ))}
      </TableHeader>
      <TableBody items={indexed} {...(leer ? { renderEmptyState: () => leer } : {})}>
        {(item) => (
          <Row
            id={item.id}
            columns={spalten}
            className="focus-ring border-b border-separator outline-none last:border-b-0 data-[hovered]:bg-fill data-[focus-visible]:-outline-offset-2"
          >
            {(s) => (
              <Cell
                className={`focus-ring px-5 py-3 align-top outline-none data-[focus-visible]:-outline-offset-2 ${s.className ?? ""}`}
              >
                {s.zelle(item.row)}
              </Cell>
            )}
          </Row>
        )}
      </TableBody>
    </AriaTable>
  );
}
