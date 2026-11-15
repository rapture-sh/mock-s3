import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { formatDuration, formatExact, formatRelative } from "@/lib/format"
import { cn } from "@/lib/utils"
import type { RequestEntry } from "@/types/admin"

/**
 * The status code, with weight rather than colour doing most of the work.
 *
 * Status is the only thing on this screen that gets colour at all, and only once it is
 * a problem — a wall of green 200s teaches you nothing, while a single amber 404 in a
 * list of them is the thing you came to find.
 */
function Status({ status }: { status: number }) {
  return (
    <span
      className={cn(
        "font-mono text-[12px] tabular-nums",
        status >= 500 && "font-medium text-red-600 dark:text-red-400",
        status >= 400 && status < 500 && "font-medium text-amber-600 dark:text-amber-500",
        status < 400 && "text-muted-foreground",
      )}
    >
      {status}
    </span>
  )
}

export function RequestTable({ entries }: { entries: RequestEntry[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead className="w-14">Status</TableHead>
          <TableHead className="w-16">Method</TableHead>
          <TableHead>Path</TableHead>
          <TableHead className="w-20 text-right">Took</TableHead>
          <TableHead className="w-24">When</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {entries.map((entry) => (
          <TableRow key={`${entry.request_id ?? entry.at}-${entry.path}`}>
            <TableCell>
              <Status status={entry.status} />
            </TableCell>
            <TableCell className="font-mono text-[12px] text-muted-foreground">
              {entry.method}
            </TableCell>
            <TableCell className="min-w-0">
              <span className="block truncate font-mono text-[13px]" title={entry.path}>
                {entry.path}
                {entry.query ? <span className="text-muted-foreground">?{entry.query}</span> : null}
              </span>
            </TableCell>
            <TableCell className="text-right text-[12px] text-muted-foreground tabular-nums">
              {formatDuration(entry.duration_ms)}
            </TableCell>
            <TableCell>
              <span className="text-[12px] text-muted-foreground" title={formatExact(entry.at)}>
                {formatRelative(entry.at)}
              </span>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
