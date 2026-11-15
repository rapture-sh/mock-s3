import * as React from "react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { AdminApiError, admin } from "@/lib/admin"

/**
 * Upload one file or many.
 *
 * The key is the current prefix plus the file's own name, so dropping files while inside
 * `photos/` puts them in `photos/`. That is what a folder view implies, and quietly
 * putting them at the root instead would be a surprise.
 */
export function UploadButton({
  bucket,
  prefix,
  onUploaded,
}: {
  bucket: string
  prefix: string
  onUploaded: () => void
}) {
  const input = React.useRef<HTMLInputElement>(null)
  const [running, setRunning] = React.useState(false)

  const upload = async (files: FileList) => {
    setRunning(true)

    const chosen = Array.from(files)
    const failures: string[] = []

    for (const file of chosen) {
      try {
        await admin.uploadObject(bucket, `${prefix}${file.name}`, file)
      } catch (caught) {
        failures.push(
          caught instanceof AdminApiError ? `${file.name}: ${caught.message}` : file.name,
        )
      }
    }

    setRunning(false)
    onUploaded()

    const uploaded = chosen.length - failures.length
    if (uploaded > 0) {
      toast.success(uploaded === 1 ? `Uploaded ${chosen[0].name}` : `Uploaded ${uploaded} files`)
    }
    for (const failure of failures) {
      toast.error(failure)
    }
  }

  return (
    <>
      <input
        aria-label="Files to upload"
        className="hidden"
        multiple={true}
        onChange={(event) => {
          const { files } = event.target
          if (files && files.length > 0) {
            void upload(files)
          }
          // Cleared so choosing the same file twice in a row still fires a change.
          event.target.value = ""
        }}
        ref={input}
        type="file"
      />
      <Button disabled={running} onClick={() => input.current?.click()} size="sm" variant="outline">
        {running ? "Uploading…" : "Upload"}
      </Button>
    </>
  )
}
