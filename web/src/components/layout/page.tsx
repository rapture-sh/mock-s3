import type * as React from "react"

import { cn } from "@/lib/utils"

/**
 * The content column.
 *
 * Capped so a table does not stretch to the width of a 34" monitor and leave the eye
 * tracking across empty space to match a key to its size — but left-aligned rather than
 * centred, so it stays anchored to the sidebar instead of drifting into the middle of a
 * wide window with a gap on either side.
 */
export function Page({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div className="h-svh flex-1 overflow-y-auto">
      <div className={cn("w-full max-w-[1080px] px-9 py-9", className)}>{children}</div>
    </div>
  )
}

export function PageHeader({
  title,
  description,
  actions,
}: {
  title: React.ReactNode
  description?: React.ReactNode
  actions?: React.ReactNode
}) {
  return (
    <header className="mb-8 flex items-start justify-between gap-6">
      <div className="min-w-0">
        <h1 className="truncate font-medium text-[22px] tracking-tight">{title}</h1>
        {description ? (
          <p className="mt-1 text-[13px] text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  )
}

/**
 * An empty state, which is a place to tell someone what to do next rather than to
 * apologise for there being nothing here.
 */
export function Empty({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border/60 border-dashed px-6 py-12 text-center">
      <p className="font-medium text-sm">{title}</p>
      {children ? (
        <div className="mx-auto mt-2 max-w-md text-[13px] text-muted-foreground">{children}</div>
      ) : null}
    </div>
  )
}

/** A failure, with the server's own words rather than a generic apology. */
export function Failure({ title, detail }: { title: string; detail?: string }) {
  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 px-6 py-10 text-center">
      <p className="font-medium text-sm">{title}</p>
      {detail ? <p className="mt-1 text-[13px] text-muted-foreground">{detail}</p> : null}
    </div>
  )
}

/** A label and value pair, for the detail strips above a table. */
export function Stat({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div>
      <div className="text-[11px] text-muted-foreground/70">{label}</div>
      <div className="mt-0.5 text-sm tabular-nums">{value}</div>
    </div>
  )
}
