import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from "recharts";
import { getTenantOverview, listWorkspaces, getWorkspace } from "@/lib/api";
import type { TenantOverview, Workspace } from "@/types/api";
import { Card, CardContent } from "@/components/ui/Card";
import { Panel, Stat, UsageBar } from "@/components/ui/Panel";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { Button } from "@/components/ui/Button";

/** Shared by every chart on the page so tooltips do not drift apart between panels. */
export const CHART_TOOLTIP_STYLE = {
  backgroundColor: "var(--color-card)",
  borderColor: "var(--color-border)",
  borderRadius: "var(--radius-md)",
  color: "var(--color-card-foreground)",
  fontSize: 12,
} as const;

interface WorkspaceChartDatum {
  name: string;
  entities: number;
}

export function DashboardPage() {
  const [overview, setOverview] = useState<TenantOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [chartData, setChartData] = useState<WorkspaceChartDatum[]>([]);

  useEffect(() => {
    async function load() {
      setLoading(true);
      setError(null);
      try {
        const [ov, wsList] = await Promise.all([getTenantOverview(), listWorkspaces()]);
        setOverview(ov);

        const details = await Promise.all(
          wsList.map(async (ws: Workspace) => {
            try {
              const detail = await getWorkspace(ws.id);
              return { name: ws.name, entities: detail.entity_count };
            } catch {
              return { name: ws.name, entities: 0 };
            }
          }),
        );
        setChartData(details);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load");
      } finally {
        setLoading(false);
      }
    }
    load();
  }, []);

  if (loading) return <PageSkeleton />;

  if (error || !overview) {
    return (
      <div className="p-6">
        <Card>
          <CardContent className="pt-6">
            <p className="text-sm text-destructive">{error ?? "Failed to load"}</p>
            <Button className="mt-4" size="sm" onClick={() => window.location.reload()}>
              Retry
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  const { usage, plan, max_workspaces } = overview;

  const roleCounts = overview.members.reduce<Record<string, number>>((acc, m) => {
    acc[m.role] = (acc[m.role] ?? 0) + 1;
    return acc;
  }, {});
  const roleData = Object.entries(roleCounts).map(([role, count]) => ({ role, count }));

  const busiest = chartData.toSorted((a, b) => b.entities - a.entities)[0];
  const mean = chartData.length
    ? Math.round(chartData.reduce((sum, d) => sum + d.entities, 0) / chartData.length)
    : 0;

  return (
    <div className="space-y-4 p-6">
      <div className="flex flex-wrap items-end justify-between gap-2">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Dashboard</h1>
          <p className="text-sm text-muted-foreground">Organization overview and analytics.</p>
        </div>
        <span className="rounded-full border border-border bg-secondary px-2.5 py-1 text-xs font-medium text-secondary-foreground">
          Plan: {plan ?? "Free"}
        </span>
      </div>

      {/* Stat row */}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Panel title="Workspaces">
          <Stat value={usage.workspace_count} limit={max_workspaces} />
          <UsageBar value={usage.workspace_count} limit={max_workspaces} />
        </Panel>
        <Panel title="Members">
          <Stat value={usage.member_count} caption={`${roleData.length} distinct roles`} />
        </Panel>
        <Panel title="Entities">
          <Stat value={usage.entity_count} caption="across all workspaces" />
        </Panel>
        <Panel title="Busiest workspace">
          <Stat
            value={busiest ? busiest.entities : 0}
            caption={busiest ? busiest.name : "no workspaces yet"}
          />
        </Panel>
      </div>

      {/* Chart row */}
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Panel
          title="Entities per workspace"
          className="lg:col-span-2"
          actions={
            chartData.length > 0 ? (
              <span className="text-xs text-muted-foreground tabular-nums">mean {mean}</span>
            ) : undefined
          }
        >
          {chartData.length === 0 ? (
            <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
              No workspaces yet.{" "}
              <Link to="/workspaces" className="ml-1 text-link hover:underline">
                Create one
              </Link>
            </div>
          ) : (
            <div className="h-64 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={chartData} margin={{ top: 8, right: 8, left: 0, bottom: 8 }}>
                  <CartesianGrid
                    strokeDasharray="3 3"
                    stroke="var(--color-border)"
                    vertical={false}
                  />
                  <XAxis
                    dataKey="name"
                    stroke="var(--color-border)"
                    tick={{ fill: "var(--color-muted-foreground)", fontSize: 11 }}
                  />
                  <YAxis
                    allowDecimals={false}
                    stroke="var(--color-border)"
                    tick={{ fill: "var(--color-muted-foreground)", fontSize: 11 }}
                    width={36}
                  />
                  <Tooltip
                    contentStyle={CHART_TOOLTIP_STYLE}
                    labelStyle={{ color: "var(--color-card-foreground)" }}
                    cursor={{ fill: "var(--color-accent)", opacity: 0.1 }}
                  />
                  <Bar
                    dataKey="entities"
                    name="Entities"
                    fill="var(--color-primary)"
                    radius={[2, 2, 0, 0]}
                  />
                </BarChart>
              </ResponsiveContainer>
            </div>
          )}
        </Panel>

        <Panel title="Members by role">
          {roleData.length === 0 ? (
            <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
              No members yet.
            </div>
          ) : (
            <div className="h-64 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart
                  data={roleData}
                  layout="vertical"
                  margin={{ top: 8, right: 8, left: 0, bottom: 8 }}
                >
                  <CartesianGrid
                    strokeDasharray="3 3"
                    stroke="var(--color-border)"
                    horizontal={false}
                  />
                  <XAxis
                    type="number"
                    allowDecimals={false}
                    stroke="var(--color-border)"
                    tick={{ fill: "var(--color-muted-foreground)", fontSize: 11 }}
                  />
                  <YAxis
                    type="category"
                    dataKey="role"
                    stroke="var(--color-border)"
                    tick={{ fill: "var(--color-muted-foreground)", fontSize: 11 }}
                    width={60}
                  />
                  <Tooltip
                    contentStyle={CHART_TOOLTIP_STYLE}
                    labelStyle={{ color: "var(--color-card-foreground)" }}
                    cursor={{ fill: "var(--color-accent)", opacity: 0.1 }}
                  />
                  <Bar
                    dataKey="count"
                    name="Members"
                    fill="var(--color-primary)"
                    radius={[0, 2, 2, 0]}
                  />
                </BarChart>
              </ResponsiveContainer>
            </div>
          )}
        </Panel>
      </div>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {[
          { to: "/workspaces", title: "Manage Workspaces", body: "Create and open workspaces" },
          { to: "/members", title: "Manage Members", body: "Invite and manage team members" },
          { to: "/schemas", title: "View Schemas", body: "Browse and create schemas" },
        ].map((link) => (
          <Link key={link.to} to={link.to}>
            <Card className="cursor-pointer transition-colors hover:bg-accent/5">
              <CardContent className="pt-6">
                <p className="font-medium">{link.title}</p>
                <p className="text-sm text-muted-foreground">{link.body}</p>
              </CardContent>
            </Card>
          </Link>
        ))}
      </div>
    </div>
  );
}
