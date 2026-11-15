import { Check, Copy, Download } from "lucide-react"
import * as React from "react"
import { toast } from "sonner"

import { ObjectPreview } from "@/components/objects/object-preview"
import { Button } from "@/components/ui/button"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { Skeleton } from "@/components/ui/skeleton"
import { useAdmin } from "@/hooks/use-admin"
import { AdminApiError, admin } from "@/lib/admin"
import { formatBytes, formatExact, formatRelative } from "@/lib/format"

/** A label and value row, with the value selectable. */
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[6.5rem_1fr] items-baseline gap-3 py-1.5">
      <div className="text-[11.5px] text-muted-foreground/70">{label}</div>
      <div className="min-w-0 break-words text-[13px]">{children}</div>
    </div>
  )
}

/**
 * Copy something a developer is about to paste into a terminal or a test.
 *
 * Shows what it copied rather than a generic tick, because the point is usually to
 * confirm you grabbed the right one of two similar strings.
 */
function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = React.useState(false)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      toast.error("The browser would not allow copying.")
    }
  }

  return (
    <button
      aria-label={`Copy ${label}`}
      className="group flex w-full items-center gap-2 rounded-md border border-border/50 bg-muted/20 px-2.5 py-1.5 text-left transition-colors hover:border-border hover:bg-muted/40"
      onClick={copy}
      type="button"
    >
      <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{value}</span>
      {copied ? (
        <Check aria-hidden="true" className="size-3.5 shrink-0" strokeWidth={2} />
      ) : (
        <Copy
          aria-hidden="true"
          className="size-3.5 shrink-0 text-muted-foreground/60 group-hover:text-foreground"
          strokeWidth={1.75}
        />
      )}
    </button>
  )
}

export function ObjectSheet({
  bucket,
  objectKey,
  onClose,
  onDeleted,
}: {
  bucket: string
  objectKey: string | undefined
  onClose: () => void
  onDeleted: () => void
}) {
  const open = objectKey !== undefined
  const detail = useAdmin(
    `object:${bucket}:${objectKey}`,
    () => admin.object(bucket, objectKey ?? ""),
    { enabled: open },
  )

  const remove = async () => {
    if (!objectKey) {
      return
    }

    try {
      await admin.deleteObject(bucket, objectKey)
      toast.success(`Deleted ${objectKey}`)
      onDeleted()
      onClose()
    } catch (caught) {
      toast.error(caught instanceof AdminApiError ? caught.message : "mock-s3 is not answering.")
    }
  }

  return (
    <Sheet onOpenChange={(next) => !next && onClose()} open={open}>
      <SheetContent className="w-full gap-0 sm:max-w-xl">
        <SheetHeader className="border-border/50 border-b">
          <SheetTitle className="break-all font-mono text-[14px] font-medium">
            {objectKey}
          </SheetTitle>
          <SheetDescription className="text-[12px]">
            {detail.data
              ? `${formatBytes(detail.data.size)} · ${detail.data.content_type}`
              : "Loading"}
          </SheetDescription>
        </SheetHeader>

        <div className="flex-1 space-y-6 overflow-y-auto px-4 py-5">
          {detail.isLoading || !objectKey ? (
            <Skeleton className="h-48 w-full" />
          ) : detail.error ? (
            <div className="rounded-md border border-border/50 px-4 py-8 text-center text-[13px] text-muted-foreground">
              {detail.error.message}
            </div>
          ) : detail.data ? (
            <>
              <ObjectPreview
                bucket={bucket}
                contentType={detail.data.content_type}
                objectKey={objectKey}
                size={detail.data.size}
              />

              <div>
                <div className="mb-2 text-[11.5px] text-muted-foreground/70">Reach it</div>
                <div className="space-y-1.5">
                  <CopyButton label="the S3 URI" value={`s3://${bucket}/${objectKey}`} />
                  <CopyButton
                    label="the object URL"
                    value={`${window.location.protocol}//${window.location.hostname}:9000/${bucket}/${objectKey}`}
                  />
                </div>
              </div>

              <div className="border-border/50 border-t pt-2">
                <Field label="Size">
                  {formatBytes(detail.data.size)}
                  <span className="ml-1.5 text-muted-foreground/60 tabular-nums">
                    ({detail.data.size.toLocaleString()} bytes)
                  </span>
                </Field>
                <Field label="Type">
                  <span className="font-mono text-[12px]">{detail.data.content_type}</span>
                </Field>
                <Field label="ETag">
                  <span className="font-mono text-[12px]">{detail.data.etag}</span>
                </Field>
                <Field label="Modified">
                  <span title={formatExact(detail.data.last_modified)}>
                    {formatRelative(detail.data.last_modified)}
                  </span>
                </Field>
                {Object.entries(detail.data.metadata).map(([name, value]) => (
                  <Field key={name} label={`x-amz-meta-${name}`}>
                    <span className="font-mono text-[12px]">{value}</span>
                  </Field>
                ))}
              </div>

              {detail.data.versions.length > 1 ? (
                <div className="border-border/50 border-t pt-4">
                  <div className="mb-2 text-[11.5px] text-muted-foreground/70">
                    Versions ({detail.data.versions.length})
                  </div>
                  <div className="space-y-1">
                    {detail.data.versions.map((version) => (
                      <div
                        className="flex items-baseline justify-between gap-3 text-[12px]"
                        key={version.version_id}
                      >
                        <span className="truncate font-mono">{version.version_id}</span>
                        <span className="shrink-0 text-muted-foreground">
                          {version.is_delete_marker ? "delete marker" : formatBytes(version.size)}
                          {version.is_latest ? " · current" : ""}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}
            </>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-2 border-border/50 border-t px-4 py-3">
          <Button
            onClick={() => {
              if (objectKey) {
                window.location.href = admin.contentUrl(bucket, objectKey, true)
              }
            }}
            size="sm"
            variant="outline"
          >
            <Download aria-hidden="true" className="size-3.5" strokeWidth={1.75} />
            Download
          </Button>
          <Button onClick={remove} size="sm" variant="ghost">
            Delete
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
