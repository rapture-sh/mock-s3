import { Link } from "@tanstack/react-router"
import { Activity, Database, LayoutDashboard } from "lucide-react"

import { useAdmin } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"
import { cn } from "@/lib/utils"

/** The instance itself: the first thing to check when nothing is working. */
function InstanceStatus() {
  const { data, error } = useAdmin("health", admin.health, { pollMs: 5000 })

  const reachable = error === undefined && data !== undefined

  return (
    <div className="px-4 pt-5 pb-6">
      <div className="font-medium text-[15px] tracking-tight">mock-s3</div>
      <div className="mt-1.5 flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
        <span
          aria-hidden="true"
          className={cn(
            "size-1.5 shrink-0 rounded-full",
            reachable ? "bg-foreground/70" : "bg-muted-foreground/40",
          )}
        />
        {reachable ? (
          <span className="truncate" title={`${data.region} · ${data.backend}`}>
            {data.region}
            <span className="px-1 text-muted-foreground/40">·</span>
            <span className="font-mono">{data.backend}</span>
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

/**
 * Three destinations, and nothing else.
 *
 * The bucket list used to live here too, which meant opening a bucket highlighted both
 * `Buckets` and the bucket, two selected things in one nav, for one location. The
 * bucket list belongs on the Buckets page, where it is the content.
 */
const NAV: NavItem[] = [
  { to: "/", label: "Overview", icon: LayoutDashboard },
  { to: "/buckets", label: "Buckets", icon: Database },
  { to: "/requests", label: "Requests", icon: Activity },
]

export function Sidebar() {
  return (
    <aside className="flex h-svh w-[210px] shrink-0 flex-col border-border/50 border-r">
      <InstanceStatus />

      <nav className="flex-1 space-y-0.5 px-2.5">
        {NAV.map((item) => {
          const Icon = item.icon

          return (
            <Link
              activeOptions={{ exact: item.to === "/" }}
              activeProps={{ "data-active": "true" }}
              className={cn(
                "flex items-center gap-2.5 rounded-md px-2.5 py-[7px] text-[13.5px]",
                "text-muted-foreground transition-colors",
                "hover:bg-accent/60 hover:text-foreground",
                "data-[active=true]:bg-accent data-[active=true]:text-foreground",
              )}
              key={item.to}
              to={item.to}
            >
              <Icon aria-hidden="true" className="size-[15px] shrink-0" strokeWidth={1.75} />
              {item.label}
            </Link>
          )
        })}
      </nav>

      <div className="px-4 py-3.5">
        <a
          className="text-[11px] text-muted-foreground/50 transition-colors hover:text-muted-foreground"
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
