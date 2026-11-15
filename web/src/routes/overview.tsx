import { Link } from "@tanstack/react-router"

import { Empty, Failure, Page, PageHeader, Stat } from "@/components/layout/page"
import { RequestTable } from "@/components/requests/request-table"
import { Skeleton } from "@/components/ui/skeleton"
import { useAdmin } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"
import { formatBytes } from "@/lib/format"

const RECENT = 60_000

export function OverviewPage() {
  const buckets = useAdmin("buckets", admin.buckets, { pollMs: 5000 })
  const requests = useAdmin("requests:100", () => admin.requests(100), { pollMs: 2000 })

  const objects = buckets.data?.reduce((total, bucket) => total + bucket.objects, 0)
  const bytes = buckets.data?.reduce((total, bucket) => total + bucket.bytes, 0)

  const lastMinute = requests.data?.filter(
    (entry) => Date.now() - new Date(entry.at).getTime() < RECENT,
  ).length

  return (
    <Page>
      <PageHeader
        description="What this instance is holding, and what is talking to it."
        title="Overview"
      />

      <div className="mb-10 grid grid-cols-2 gap-6 border-border/60 border-y py-4 sm:grid-cols-4">
        <Stat label="Buckets" value={buckets.data?.length ?? "—"} />
        <Stat label="Objects" value={objects ?? "—"} />
        <Stat label="Stored" value={bytes === undefined ? "—" : formatBytes(bytes)} />
        <Stat label="Requests, last minute" value={lastMinute ?? "—"} />
      </div>

      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="font-medium text-sm">Recent requests</h2>
        <Link className="text-[12px] text-muted-foreground hover:text-foreground" to="/requests">
          All requests
        </Link>
      </div>

      {requests.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : requests.error ? (
        <Failure detail={requests.error.message} title="Could not reach mock-s3" />
      ) : requests.data && requests.data.length > 0 ? (
        <RequestTable entries={requests.data.slice(0, 15)} />
      ) : (
        <Empty title="No requests yet">
          Point a client at <span className="font-mono">http://localhost:9000</span>:
          <pre className="mt-3 whitespace-pre-wrap break-words rounded-md bg-muted/50 px-3 py-2 text-left font-mono text-[11.5px] leading-relaxed">
            aws s3 ls --endpoint-url http://localhost:9000
          </pre>
        </Empty>
      )}
    </Page>
  )
}
