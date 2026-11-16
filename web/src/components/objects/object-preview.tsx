import * as React from "react"

import { Skeleton } from "@/components/ui/skeleton"
import { admin } from "@/lib/admin"
import { formatBytes } from "@/lib/format"

/** Text is fetched into the page, so it is capped. Images stream through an `<img>`. */
const MAX_TEXT_BYTES = 256 * 1024

type Kind = "image" | "text" | "none"

/**
 * What can usefully be shown for a content type.
 *
 * Deliberately conservative: anything not recognised says so and offers a download,
 * rather than rendering bytes as text and showing a screen of mojibake.
 */
export function previewKind(contentType: string): Kind {
  const type = contentType.toLowerCase()

  if (type.startsWith("image/")) {
    return "image"
  }

  if (
    type.startsWith("text/") ||
    type.includes("json") ||
    type.includes("xml") ||
    type.includes("javascript") ||
    type.includes("x-www-form-urlencoded")
  ) {
    return "text"
  }

  return "none"
}

function TextPreview({ bucket, objectKey }: { bucket: string; objectKey: string }) {
  const [text, setText] = React.useState<string>()
  const [failed, setFailed] = React.useState(false)

  React.useEffect(() => {
    let current = true
    setText(undefined)
    setFailed(false)

    admin
      .contentText(bucket, objectKey, MAX_TEXT_BYTES)
      .then((loaded) => {
        if (current) {
          setText(loaded)
        }
      })
      .catch(() => {
        if (current) {
          setFailed(true)
        }
      })

    return () => {
      current = false
    }
  }, [bucket, objectKey])

  if (failed) {
    return <PreviewNote>This object could not be read.</PreviewNote>
  }

  if (text === undefined) {
    return <Skeleton className="h-40 w-full" />
  }

  return (
    <pre className="max-h-[420px] overflow-auto rounded-md border border-border/50 bg-muted/20 p-3 font-mono text-[12px] leading-relaxed">
      {text}
    </pre>
  )
}

function PreviewNote({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-md border border-border/50 border-dashed px-4 py-10 text-center text-[13px] text-muted-foreground">
      {children}
    </div>
  )
}

export function ObjectPreview({
  bucket,
  objectKey,
  contentType,
  size,
}: {
  bucket: string
  objectKey: string
  contentType: string
  size: number
}) {
  const kind = previewKind(contentType)

  if (size === 0) {
    return <PreviewNote>This object is empty.</PreviewNote>
  }

  if (kind === "image") {
    return (
      <div className="flex items-center justify-center rounded-md border border-border/50 bg-muted/20 p-4">
        <img
          alt={objectKey}
          className="max-h-[420px] max-w-full object-contain"
          src={admin.contentUrl(bucket, objectKey)}
        />
      </div>
    )
  }

  if (kind === "text") {
    return <TextPreview bucket={bucket} objectKey={objectKey} />
  }

  return (
    <PreviewNote>
      No preview for {contentType}. {formatBytes(size)}, download it to open.
    </PreviewNote>
  )
}
