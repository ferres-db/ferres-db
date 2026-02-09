import { useAnalytics } from '@/hooks/useStats';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import {
  PieChart,
  Pie,
  Cell,
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from 'recharts';
import { Activity, Gauge, ShieldAlert, BarChart3 } from 'lucide-react';
import { format } from 'date-fns';

const TIER_COLORS = ['#f97316', '#eab308', '#3b82f6']; // Hot, Warm, Cold

export const Analytics = () => {
  const { data, isLoading, isError, error } = useAnalytics();

  if (isError) {
    return (
      <div className="space-y-8">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Analytics & Reports</h1>
          <p className="mt-1 text-sm text-gray-400">System-wide metrics and health</p>
        </div>
        <Card>
          <CardContent className="p-8 text-center">
            <p className="text-gray-400">
              Failed to load analytics: {(error as Error)?.message ?? 'Unknown error'}
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  const totalVectors = data?.tombstones?.total_points ?? 0;
  const p50Ms = data?.latency?.p50_ms ?? 0;
  const totalPoints = data?.tombstones?.total_points ?? 0;
  const tombstoneCount = data?.tombstones?.total_count ?? 0;
  const tombstonePct =
    totalPoints > 0 ? ((tombstoneCount / totalPoints) * 100).toFixed(2) : null;
  const tierDistribution = data?.tier_distribution;
  const tierPieData =
    tierDistribution && (tierDistribution.hot + tierDistribution.warm + tierDistribution.cold) > 0
      ? [
          { name: 'Hot', value: tierDistribution.hot },
          { name: 'Warm', value: tierDistribution.warm },
          { name: 'Cold', value: tierDistribution.cold },
        ].filter((d) => d.value > 0)
      : [];
  const latencyPerMinute = data?.latency?.latency_per_minute ?? [];
  const latencyChartData = latencyPerMinute.map((b) => ({
    time: format(new Date(b.timestamp * 1000), 'HH:mm'),
    timestamp: b.timestamp,
    p50: b.p50_ms,
    avg: b.avg_ms,
  }));
  const circuitState = data?.circuit_breaker?.state ?? 'unknown';
  const failureCount = data?.circuit_breaker?.failure_count ?? 0;

  const circuitVariant =
    circuitState === 'closed' ? 'success' : circuitState === 'open' ? 'danger' : 'warning';
  const circuitLabel =
    circuitState === 'closed'
      ? 'Closed'
      : circuitState === 'open'
        ? 'Open'
        : circuitState === 'half_open'
          ? 'Half Open'
          : circuitState;

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-gray-50">
          Analytics & Reports
        </h1>
        <p className="mt-1 text-sm text-gray-400">System-wide metrics and health</p>
      </div>

      {/* KPI cards */}
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Vectors</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <Activity className="h-4 w-4 text-orange-500" />
            </div>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {totalVectors.toLocaleString()}
              </p>
            )}
          </CardContent>
        </Card>

        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Latency P50 (ms)</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <Gauge className="h-4 w-4 text-orange-500" />
            </div>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {typeof p50Ms === 'number' ? p50Ms.toFixed(2) : p50Ms}
              </p>
            )}
          </CardContent>
        </Card>

        <Card className="overflow-hidden sm:col-span-2 lg:col-span-1">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Index Health (Tombstones %)</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <BarChart3 className="h-4 w-4 text-orange-500" />
            </div>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : tombstonePct !== null ? (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">{tombstonePct}%</p>
            ) : (
              <p className="text-sm text-gray-400">No points</p>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Circuit breaker */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium text-gray-400">Storage Circuit Breaker</CardTitle>
          <Badge variant={circuitVariant}>{circuitLabel}</Badge>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <Skeleton className="h-6 w-24 rounded" />
          ) : (
            <div className="flex items-center gap-2">
              <ShieldAlert className="h-4 w-4 text-gray-400" />
              <span className="text-sm text-gray-400">
                State: {circuitLabel}
                {failureCount > 0 && ` · Failures: ${failureCount}`}
              </span>
            </div>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        {/* Pie: tier distribution */}
        <Card>
          <CardHeader>
            <CardTitle>Memory by Tier (Hot / Warm / Cold)</CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-[250px] w-full rounded" />
            ) : tierPieData.length > 0 ? (
              <ResponsiveContainer width="100%" height={250}>
                <PieChart>
                  <Pie
                    data={tierPieData}
                    cx="50%"
                    cy="50%"
                    innerRadius={60}
                    outerRadius={90}
                    paddingAngle={2}
                    dataKey="value"
                    nameKey="name"
                    label={({ name, value }) => `${name}: ${value}`}
                  >
                    {tierPieData.map((_, index) => (
                      <Cell key={`cell-${index}`} fill={TIER_COLORS[index % TIER_COLORS.length]} />
                    ))}
                  </Pie>
                  <Tooltip
                    contentStyle={{
                      backgroundColor: '#2d2d2d',
                      border: '1px solid #3f3f3f',
                      borderRadius: '8px',
                    }}
                    formatter={(value: number | undefined) => [
                      value != null ? value.toLocaleString() : '0',
                      'Points',
                    ]}
                  />
                  <Legend />
                </PieChart>
              </ResponsiveContainer>
            ) : (
              <p className="py-12 text-center text-sm text-gray-400">No tier data available</p>
            )}
          </CardContent>
        </Card>

        {/* Line: latency over time */}
        <Card>
          <CardHeader>
            <CardTitle>Search Latency (last 24h)</CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-[250px] w-full rounded" />
            ) : latencyChartData.length > 0 ? (
              <ResponsiveContainer width="100%" height={250}>
                <LineChart data={latencyChartData}>
                  <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                  <XAxis dataKey="time" stroke="#9ca3af" />
                  <YAxis stroke="#9ca3af" />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: '#2d2d2d',
                      border: '1px solid #3f3f3f',
                      borderRadius: '8px',
                    }}
                    labelStyle={{ color: '#f9fafb' }}
                  />
                  <Legend />
                  <Line
                    type="monotone"
                    dataKey="p50"
                    stroke="#f97316"
                    strokeWidth={2}
                    name="P50 (ms)"
                    dot={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="avg"
                    stroke="#eab308"
                    strokeWidth={2}
                    name="Avg (ms)"
                    dot={false}
                  />
                </LineChart>
              </ResponsiveContainer>
            ) : (
              <p className="py-12 text-center text-sm text-gray-400">
                No latency data in the last 24h
              </p>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
};
