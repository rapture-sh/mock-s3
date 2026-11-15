import * as React from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useAction } from "@/hooks/use-admin"
import { admin } from "@/lib/admin"
import { validateBucketName } from "@/lib/bucket-name"

export function CreateBucketDialog({ onCreated }: { onCreated: () => void }) {
  const [open, setOpen] = React.useState(false)
  const [name, setName] = React.useState("")
  const create = useAction(admin.createBucket)

  // Shown as soon as there is something to say. Gating this on blur would leave the
  // submit button disabled with no stated reason, which is the worst of both.
  const problem = name === "" ? undefined : validateBucketName(name)
  const canSubmit = name !== "" && problem === undefined && !create.isRunning

  const reset = () => {
    setName("")
    create.reset()
  }

  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!canSubmit) {
      return
    }

    if (await create.run(name)) {
      onCreated()
      setOpen(false)
      reset()
    }
  }

  return (
    <Dialog
      onOpenChange={(next) => {
        setOpen(next)
        if (!next) {
          reset()
        }
      }}
      open={open}
    >
      <DialogTrigger render={<Button size="sm" />}>New bucket</DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>New bucket</DialogTitle>
            <DialogDescription>
              Names follow the real S3 rules, so a name that works here works against AWS.
            </DialogDescription>
          </DialogHeader>
          <div className="py-5">
            <Label className="text-[13px]" htmlFor="bucket-name">
              Name
            </Label>
            <Input
              aria-describedby={problem ? "bucket-name-problem" : "bucket-name-hint"}
              aria-invalid={problem !== undefined}
              autoComplete="off"
              autoFocus={true}
              className="mt-1.5 font-mono text-[13px]"
              id="bucket-name"
              name="bucket-name"
              onChange={(event) => {
                setName(event.target.value)
                create.reset()
              }}
              placeholder="my-test-fixtures"
              spellCheck={false}
              value={name}
            />
            {problem ? (
              <p className="mt-2 text-[12px] text-destructive" id="bucket-name-problem">
                {problem}
              </p>
            ) : (
              <p className="mt-2 text-[12px] text-muted-foreground" id="bucket-name-hint">
                Lowercase letters, numbers, hyphens and dots. 3 to 63 characters.
              </p>
            )}
            {create.error ? (
              <p className="mt-2 text-[12px] text-destructive">
                {create.error.code}: {create.error.message}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <DialogClose render={<Button size="sm" type="button" variant="ghost" />}>
              Cancel
            </DialogClose>
            <Button disabled={!canSubmit} size="sm" type="submit">
              {create.isRunning ? "Creating…" : "Create bucket"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
