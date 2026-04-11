import { Button } from "@/components/ui/button"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { RecentRequestsTable } from "@/features/dashboard/RecentRequestsTable"
import { formatInteger } from "@/features/dashboard/format"
import type { DashboardRequestItem } from "@/features/dashboard/types"
import { m } from "@/paraglide/messages.js"

type DataTableProps = {
  items: DashboardRequestItem[]
  page: number
  totalPages: number
  totalRequests: number
  pageSize: number
  loading: boolean
  scrollKey: string
  onPrevPage: () => void
  onNextPage: () => void
  onSelectItem?: (item: DashboardRequestItem) => void
}

export function DataTable({
  items,
  page,
  totalPages,
  totalRequests,
  pageSize,
  loading,
  scrollKey,
  onPrevPage,
  onNextPage,
  onSelectItem,
}: DataTableProps) {
  return (
    <div className="px-4 lg:px-6">
      <Card data-slot="dashboard-recent">
        <CardHeader>
          <CardTitle className="text-base">{m.dashboard_recent_title()}</CardTitle>
          <CardDescription>
            {m.dashboard_recent_desc({
              pageSize: String(pageSize),
              total: formatInteger(totalRequests),
            })}
          </CardDescription>
          <CardAction className="flex flex-col items-end gap-2">
            <div className="flex items-center gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={page <= 1 || loading}
                onClick={onPrevPage}
              >
                {m.dashboard_prev_page()}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={page >= totalPages || loading}
                onClick={onNextPage}
              >
                {m.dashboard_next_page()}
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">
              {m.dashboard_page_indicator({
                page: String(page),
                totalPages: String(totalPages),
              })}
            </p>
          </CardAction>
        </CardHeader>
        <CardContent className="pt-0">
          {items.length ? (
            <RecentRequestsTable
              items={items}
              scrollKey={scrollKey}
              onSelectItem={onSelectItem}
            />
          ) : (
            <p className="text-sm text-muted-foreground">{m.dashboard_no_data()}</p>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
