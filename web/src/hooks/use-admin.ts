import * as React from "react"

import { AdminApiError } from "@/lib/admin"

export type Query<T> = {
  data: T | undefined
  error: AdminApiError | undefined
  /** True only on the first load, so a poll does not flash a skeleton over good data. */
  isLoading: boolean
  refresh: () => void
}

type Options = {
  /** Poll this often, in milliseconds. Omit for a one-off read. */
  pollMs?: number
  /** Skip the request entirely — for a query that depends on a route param. */
  enabled?: boolean
}

/**
 * Read something from the admin API, with optional polling.
 *
 * Deliberately small rather than a data-fetching library: there are five endpoints, they
 * are all on localhost, and the console is a viewer. What it does need is to not flash a
 * loading state every two seconds while polling, and to drop a response that arrives
 * after its query stopped being the current one.
 */
export function useAdmin<T>(
  key: string,
  fetcher: () => Promise<T>,
  { pollMs, enabled = true }: Options = {},
): Query<T> {
  const [data, setData] = React.useState<T>()
  const [error, setError] = React.useState<AdminApiError>()
  const [isLoading, setIsLoading] = React.useState(enabled)
  const [nonce, setNonce] = React.useState(0)

  // Held in a ref so changing the fetcher identity on every render does not restart the
  // effect; `key` is what says the query has actually changed.
  const fetcherRef = React.useRef(fetcher)
  fetcherRef.current = fetcher

  const refresh = React.useCallback(() => {
    setNonce((current) => current + 1)
  }, [])

  // `key` and `nonce` look unused in here because they are signals rather than values:
  // `key` says the query itself changed and `nonce` says someone asked for a refresh.
  // The fetcher is held in a ref instead of listed, because it is a closure rebuilt every
  // render and depending on it would restart the poll continuously.
  // biome-ignore lint/correctness/useExhaustiveDependencies: key and nonce are refetch signals, not values read in the effect
  React.useEffect(() => {
    if (!enabled) {
      setIsLoading(false)
      return undefined
    }

    let current = true

    const read = async () => {
      try {
        const next = await fetcherRef.current()
        if (current) {
          setData(next)
          setError(undefined)
        }
      } catch (caught) {
        if (current) {
          setError(
            caught instanceof AdminApiError
              ? caught
              : new AdminApiError(0, "Unreachable", "mock-s3 is not answering."),
          )
        }
      } finally {
        if (current) {
          setIsLoading(false)
        }
      }
    }

    void read()

    if (pollMs === undefined) {
      return () => {
        current = false
      }
    }

    const timer = window.setInterval(read, pollMs)

    return () => {
      current = false
      window.clearInterval(timer)
    }
  }, [key, enabled, pollMs, nonce])

  return { data, error, isLoading, refresh }
}

/**
 * Run a write and report how it went, so a button can disable itself and a form can show
 * why the server said no.
 */
export function useAction<Args extends unknown[]>(action: (...args: Args) => Promise<unknown>) {
  const [isRunning, setIsRunning] = React.useState(false)
  const [error, setError] = React.useState<AdminApiError>()

  const run = React.useCallback(
    async (...args: Args): Promise<boolean> => {
      setIsRunning(true)
      setError(undefined)
      try {
        await action(...args)
        return true
      } catch (caught) {
        setError(
          caught instanceof AdminApiError
            ? caught
            : new AdminApiError(0, "Unreachable", "mock-s3 is not answering."),
        )
        return false
      } finally {
        setIsRunning(false)
      }
    },
    [action],
  )

  const reset = React.useCallback(() => {
    setError(undefined)
  }, [])

  return { run, isRunning, error, reset }
}
