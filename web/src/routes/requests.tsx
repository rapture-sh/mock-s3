import { Empty, Failure, Page, PageHeader } from "@/components/layout/page"
import { RequestTable } from "@/components/requests/request-table"
import { Skeleton } from "@/components/ui/skeleton"
import { useAdmin } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"

export function RequestsPage() {
  const requests = useAdmin("requests:500", () => admin.requests(500), { pollMs: 2000 })

  return (
    <Page>
      <PageHeader description="What your client actually sent, newest first." title="Requests" />

      {requests.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : requests.error ? (
        <Failure detail={requests.error.message} title="Could not read the request log" />
      ) : requests.data && requests.data.length > 0 ? (
        <RequestTable entries={requests.data} />
      ) : (
        <Empty title="Nothing yet">
          Point a client at <span className="font-mono">http://localhost:9000</span> and its
          requests show up here. The log is in memory and clears when mock-s3 restarts.
        </Empty>
      )}
    </Page>
  )
}
