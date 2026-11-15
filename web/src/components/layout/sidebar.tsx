import { Link, useRouterState } from "@tanstack/react-router"
import { Activity, Database, LayoutDashboard } from "lucide-react"

import { useAdmin } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"
import { cn } from "@/lib/utils"

/** The instance header: what you check first when nothing is working. */
function InstanceStatus() {
  const { data, error } = useAdmin("health", admin.health, { pollMs: 5000 })

  const reachable = error === undefined && data !== undefined

  return (
    <div className="px-3 py-4">
      <div className="flex items-baseline gap-2">
        <span className="font-medium text-[15px] tracking-tight">mock-s3</span>
      </div>
      <div className="mt-1 flex items-center gap-1.5 text-muted-foreground text-xs">
        <span
          aria-hidden="true"
          className={cn(
            "size-1.5 rounded-full",
            reachable ? "bg-foreground" : "bg-muted-foreground/40",
          )}
        />
        {reachable ? (
          <span className="truncate">
            {data.region}
            <span className="px-1 text-muted-foreground/50">/</span>
            <span className="font-mono text-[11px]">{data.backend}</span>
          </span>
        ) : (
          <span>not answering</span>
        )}
      </div>
    </div>
  )
}

type NavItem = {
  to: string
  label: string
  icon: typeof Database
}

const NAV: NavItem[] = [
  { to: "/", label: "Overview", icon: LayoutDashboard },
  { to: "/buckets", label: "Buckets", icon: Database },
  { to: "/requests", label: "Requests", icon: Activity },
]

function NavLink({ item }: { item: NavItem }) {
  const Icon = item.icon

  return (
    <Link
      activeOptions={{ exact: item.to === "/" }}
      activeProps={{ "data-active": "true" }}
      className={cn(
        "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-muted-foreground text-sm",
        "transition-colors hover:bg-accent hover:text-foreground",
        "data-[active=true]:bg-accent data-[active=true]:font-medium",
        "data-[active=true]:text-foreground",
      )}
      to={item.to}
    >
      <Icon aria-hidden="true" className="size-4 shrink-0" strokeWidth={1.75} />
      {item.label}
    </Link>
  )
}

/**
 * Buckets listed in the sidebar rather than behind a click.
 *
 * A test run has a handful of buckets and you are usually going back to the same one, so
 * making it a destination costs a click every time for no gain. Capped, with the rest on
 * the Buckets page.
 */
function BucketList() {
  const { data } = useAdmin("buckets", admin.buckets, { pollMs: 5000 })
  const location = useRouterState({ select: (state) => state.location.pathname })

  if (!data || data.length === 0) {
    return null
  }

  const shown = data.slice(0, 8)

  return (
    <div className="mt-6">
      <div className="px-2.5 pb-1 text-[11px] text-muted-foreground/70">Buckets</div>
      <nav className="space-y-px">
        {shown.map((bucket) => {
          const to = `/buckets/${encodeURIComponent(bucket.name)}`
          const isActive = location === to

          return (
            <Link
              className={cn(
                "flex items-center justify-between gap-2 rounded-md px-2.5 py-1.5",
                "text-muted-foreground text-sm transition-colors",
                "hover:bg-accent hover:text-foreground",
                isActive && "bg-accent font-medium text-foreground",
              )}
              key={bucket.name}
              params={{ bucket: bucket.name }}
              search={{ prefix: "" }}
              to="/buckets/$bucket"
            >
              <span className="truncate font-mono text-[13px]">{bucket.name}</span>
              <span className="shrink-0 text-[11px] text-muted-foreground/60 tabular-nums">
                {bucket.objects}
              </span>
            </Link>
          )
        })}
      </nav>
      {data.length > shown.length ? (
        <Link
          className="mt-1 block px-2.5 py-1 text-[11px] text-muted-foreground/70 hover:text-foreground"
          to="/buckets"
        >
          {data.length - shown.length} more
        </Link>
      ) : null}
    </div>
  )
}

export function Sidebar() {
  return (
    <aside className="flex h-svh w-60 shrink-0 flex-col border-border/60 border-r">
      <InstanceStatus />
      <div className="flex-1 overflow-y-auto px-2 pb-4">
        <nav className="space-y-px">
          {NAV.map((item) => (
            <NavLink item={item} key={item.to} />
          ))}
        </nav>
        <BucketList />
      </div>
      <div className="border-border/60 border-t px-3 py-2.5">
        <a
          className="text-[11px] text-muted-foreground/60 transition-colors hover:text-foreground hover:underline"
          href="https://rapture.sh"
          rel="noreferrer"
          target="_blank"
        >
          by rapture.sh
        </a>
      </div>
    </aside>
  )
}
