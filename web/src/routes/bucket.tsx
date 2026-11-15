import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router"
import { ChevronRight, File, Folder } from "lucide-react"
import { toast } from "sonner"

import { BucketActions } from "@/components/buckets/bucket-actions"
import { Empty, Failure, Page, PageHeader, Stat } from "@/components/layout/page"
import { UploadButton } from "@/components/objects/upload-button"
import { Button } from "@/components/ui/button"
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
import { AdminApiError, admin } from "@/lib/admin"
import {
  formatBytes,
  formatExact,
  formatRelative,
  keyLeaf,
  prefixCrumbs,
  prefixLeaf,
} from "@/lib/format"

function Breadcrumbs({ bucket, prefix }: { bucket: string; prefix: string }) {
  const crumbs = prefixCrumbs(prefix)

  return (
    <nav aria-label="Prefix" className="flex min-w-0 items-center gap-1 text-[13px]">
      <Link
        className="font-mono text-muted-foreground hover:text-foreground hover:underline"
        params={{ bucket }}
        search={{ prefix: "" }}
        to="/buckets/$bucket"
      >
        {bucket}
      </Link>
      {crumbs.map((crumb, index) => (
        <span className="flex min-w-0 items-center gap-1" key={crumb.prefix}>
          <ChevronRight aria-hidden="true" className="size-3 shrink-0 text-muted-foreground/50" />
          {index === crumbs.length - 1 ? (
            <span className="truncate font-mono">{crumb.label}</span>
          ) : (
            <Link
              className="truncate font-mono text-muted-foreground hover:text-foreground hover:underline"
              params={{ bucket }}
              search={{ prefix: crumb.prefix }}
              to="/buckets/$bucket"
            >
              {crumb.label}
            </Link>
          )}
        </span>
      ))}
    </nav>
  )
}

export function BucketPage() {
  const { bucket } = useParams({ from: "/buckets/$bucket" })
  const { prefix } = useSearch({ from: "/buckets/$bucket" })
  const navigate = useNavigate()

  const detail = useAdmin(`bucket:${bucket}`, () => admin.bucket(bucket), { pollMs: 5000 })
  const listing = useAdmin(`objects:${bucket}:${prefix}`, () => admin.objects(bucket, prefix), {
    pollMs: 5000,
  })

  const refresh = () => {
    detail.refresh()
    listing.refresh()
  }

  const removeObject = async (key: string) => {
    try {
      await admin.deleteObject(bucket, key)
      toast.success(`Deleted ${keyLeaf(key, prefix)}`)
      refresh()
    } catch (caught) {
      toast.error(caught instanceof AdminApiError ? caught.message : "mock-s3 is not answering.")
    }
  }

  if (detail.error?.code === "NoSuchBucket") {
    return (
      <Page>
        <PageHeader title={bucket} />
        <Failure
          detail="It may have been deleted, or reset while this page was open."
          title="This bucket is not here"
        />
      </Page>
    )
  }

  return (
    <Page>
      <PageHeader
        actions={
          <>
            <UploadButton bucket={bucket} onUploaded={refresh} prefix={prefix} />
            <BucketActions
              bucket={bucket}
              objects={detail.data?.objects ?? 0}
              onChanged={refresh}
              onDeleted={() => navigate({ to: "/buckets" })}
            />
          </>
        }
        title={<span className="font-mono text-[20px]">{bucket}</span>}
      />

      <div className="mb-8 grid grid-cols-2 gap-6 border-border/60 border-y py-4 sm:grid-cols-4">
        <Stat label="Objects" value={detail.data?.objects ?? "—"} />
        <Stat label="Size" value={detail.data ? formatBytes(detail.data.bytes) : "—"} />
        <Stat label="Versioning" value={detail.data?.versioning ?? "—"} />
        <Stat
          label="Created"
          value={
            detail.data ? (
              <span title={formatExact(detail.data.created_at)}>
                {formatRelative(detail.data.created_at)}
              </span>
            ) : (
              "—"
            )
          }
        />
      </div>

      <div className="mb-3 flex items-center justify-between gap-4">
        <Breadcrumbs bucket={bucket} prefix={prefix} />
        {listing.data?.is_truncated ? (
          <span className="text-[11px] text-muted-foreground">showing the first 1000 entries</span>
        ) : null}
      </div>

      {listing.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : listing.error ? (
        <Failure detail={listing.error.message} title="Could not read this bucket" />
      ) : listing.data && (listing.data.prefixes.length > 0 || listing.data.objects.length > 0) ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Key</TableHead>
              <TableHead className="w-24 text-right">Size</TableHead>
              <TableHead className="w-40">Type</TableHead>
              <TableHead className="w-28">Modified</TableHead>
              <TableHead className="w-20" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {listing.data.prefixes.map((entry) => (
              <TableRow key={entry}>
                <TableCell colSpan={4}>
                  <Link
                    className="flex items-center gap-2 font-mono text-[13px] hover:underline"
                    params={{ bucket }}
                    search={{ prefix: entry }}
                    to="/buckets/$bucket"
                  >
                    <Folder
                      aria-hidden="true"
                      className="size-3.5 shrink-0 text-muted-foreground"
                      strokeWidth={1.75}
                    />
                    {prefixLeaf(entry, prefix)}
                  </Link>
                </TableCell>
                <TableCell />
              </TableRow>
            ))}
            {listing.data.objects.map((object) => (
              <TableRow key={object.key}>
                <TableCell>
                  <span className="flex min-w-0 items-center gap-2">
                    <File
                      aria-hidden="true"
                      className="size-3.5 shrink-0 text-muted-foreground/60"
                      strokeWidth={1.75}
                    />
                    <span className="truncate font-mono text-[13px]" title={object.key}>
                      {keyLeaf(object.key, prefix) || object.key}
                    </span>
                  </span>
                </TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatBytes(object.size)}
                </TableCell>
                <TableCell className="truncate text-muted-foreground text-[12px]">
                  {object.content_type}
                </TableCell>
                <TableCell>
                  <span
                    className="text-muted-foreground text-[12px]"
                    title={formatExact(object.last_modified)}
                  >
                    {formatRelative(object.last_modified)}
                  </span>
                </TableCell>
                <TableCell>
                  <div className="flex justify-end">
                    <Button onClick={() => removeObject(object.key)} size="sm" variant="ghost">
                      Delete
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <Empty title={prefix === "" ? "This bucket is empty" : "Nothing under this prefix"}>
          Upload a file above, or write to it from your code:
          <pre className="mt-3 whitespace-pre-wrap break-words rounded-md bg-muted/50 px-3 py-2 text-left font-mono text-[11.5px] leading-relaxed">
            aws s3 cp ./file s3://{bucket}/{prefix} --endpoint-url http://localhost:9000
          </pre>
        </Empty>
      )}
    </Page>
  )
}
