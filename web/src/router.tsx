import { createRootRoute, createRoute, createRouter, Outlet } from "@tanstack/react-router"

import { Sidebar } from "@/components/layout/sidebar"
import { Toaster } from "@/components/ui/sonner"
import { BucketPage } from "@/routes/bucket"
import { BucketsPage } from "@/routes/buckets"
import { OverviewPage } from "@/routes/overview"
import { RequestsPage } from "@/routes/requests"

/**
 * Routes are declared here rather than generated from the filesystem: there are four of
 * them, and a generated route tree would add a build step and a committed artifact to
 * keep in sync for no gain at this size.
 */
const rootRoute = createRootRoute({
  component: () => (
    <div className="flex">
      <Sidebar />
      <Outlet />
      <Toaster position="bottom-right" />
    </div>
  ),
})

const overviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: OverviewPage,
})

const bucketsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/buckets",
  component: BucketsPage,
})

const bucketRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/buckets/$bucket",
  component: BucketPage,
  // The prefix lives in the URL so a folder deep in a bucket can be linked and reloaded.
  validateSearch: (search: Record<string, unknown>): { prefix: string } => ({
    prefix: typeof search.prefix === "string" ? search.prefix : "",
  }),
})

const requestsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/requests",
  component: RequestsPage,
})

const routeTree = rootRoute.addChildren([overviewRoute, bucketsRoute, bucketRoute, requestsRoute])

export const router = createRouter({ routeTree })

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router
  }
}
