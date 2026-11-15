import * as React from "react"
import { toast } from "sonner"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { AdminApiError, admin } from "@/lib/admin"

type Intent = "purge" | "delete"

/**
 * Purge and delete, each behind a confirmation that names what goes.
 *
 * They are separate on purpose, and deleting does not purge first: the server refuses to
 * delete a bucket that still holds objects, exactly as S3 does, and quietly emptying it
 * to get past that would turn one misclick into lost fixtures.
 */
export function BucketActions({
  bucket,
  objects,
  onChanged,
  onDeleted,
}: {
  bucket: string
  objects: number
  onChanged: () => void
  onDeleted?: () => void
}) {
  const [intent, setIntent] = React.useState<Intent>()
  const [running, setRunning] = React.useState(false)

  const confirm = async () => {
    setRunning(true)
    try {
      if (intent === "purge") {
        const report = await admin.purgeBucket(bucket)
        const removed = report.objects_removed ?? 0
        toast.success(removed === 1 ? "Removed 1 object" : `Removed ${removed} objects`)
        onChanged()
      }

      if (intent === "delete") {
        await admin.deleteBucket(bucket)
        toast.success(`Deleted ${bucket}`)
        onDeleted?.()
        onChanged()
      }
    } catch (caught) {
      // The server's own words: "BucketNotEmpty" says more than "something went wrong".
      toast.error(caught instanceof AdminApiError ? caught.message : "mock-s3 is not answering.")
    } finally {
      setRunning(false)
      setIntent(undefined)
    }
  }

  return (
    <>
      <Button
        disabled={running || objects === 0}
        onClick={() => setIntent("purge")}
        size="sm"
        variant="outline"
      >
        Purge
      </Button>
      <Button disabled={running} onClick={() => setIntent("delete")} size="sm" variant="outline">
        Delete
      </Button>

      <AlertDialog
        onOpenChange={(open) => !open && setIntent(undefined)}
        open={intent !== undefined}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {intent === "purge" ? `Purge ${bucket}?` : `Delete ${bucket}?`}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {intent === "purge"
                ? `This removes ${objects === 1 ? "the 1 object" : `all ${objects} objects`} in the bucket and keeps the bucket itself. It cannot be undone.`
                : objects > 0
                  ? `This bucket still holds ${objects === 1 ? "1 object" : `${objects} objects`}, so the server will refuse. Purge it first.`
                  : "This removes the bucket. It cannot be undone."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel render={<Button size="sm" variant="ghost" />}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              render={<Button size="sm" variant="destructive" />}
              onClick={confirm}
            >
              {intent === "purge" ? "Purge bucket" : "Delete bucket"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
