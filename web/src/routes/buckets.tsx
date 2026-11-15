import { Link } from "@tanstack/react-router"

import { BucketActions } from "@/components/buckets/bucket-actions"
import { CreateBucketDialog } from "@/components/buckets/create-bucket-dialog"
import { Empty, Failure, Page, PageHeader } from "@/components/layout/page"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useAdmin } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"
import { formatBytes, formatExact, formatRelative } from "@/lib/format"

export function BucketsPage() {
  const buckets = useAdmin("buckets", admin.buckets, { pollMs: 5000 })

  return (
    <Page>
      <PageHeader
        actions={<CreateBucketDialog onCreated={buckets.refresh} />}
        description="Every bucket on this instance, with what it holds."
        title="Buckets"
      />

      {buckets.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : buckets.error ? (
        <Failure detail={buckets.error.message} title="Could not read the bucket list" />
      ) : buckets.data && buckets.data.length > 0 ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead className="w-24 text-right">Objects</TableHead>
              <TableHead className="w-24 text-right">Size</TableHead>
              <TableHead className="w-32">Created</TableHead>
              <TableHead className="w-44" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {buckets.data.map((bucket) => (
              <TableRow key={bucket.name}>
                <TableCell>
                  <Link
                    className="font-mono text-[13px] hover:underline"
                    params={{ bucket: bucket.name }}
                    search={{ prefix: "" }}
                    to="/buckets/$bucket"
                  >
                    {bucket.name}
                  </Link>
                </TableCell>
                <TableCell className="text-right tabular-nums">{bucket.objects}</TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatBytes(bucket.bytes)}
                </TableCell>
                <TableCell>
                  <span className="text-muted-foreground" title={formatExact(bucket.created_at)}>
                    {formatRelative(bucket.created_at)}
                  </span>
                </TableCell>
                <TableCell>
                  <div className="flex justify-end gap-1.5">
                    <BucketActions
                      bucket={bucket.name}
                      objects={bucket.objects}
                      onChanged={buckets.refresh}
                    />
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <Empty title="No buckets yet">
          Create one above, or let your code do it:
          <pre className="mt-3 whitespace-pre-wrap break-words rounded-md bg-muted/50 px-3 py-2 text-left font-mono text-[11.5px] leading-relaxed">
            aws s3 mb s3://my-bucket --endpoint-url http://localhost:9000
          </pre>
        </Empty>
      )}
    </Page>
  )
}
