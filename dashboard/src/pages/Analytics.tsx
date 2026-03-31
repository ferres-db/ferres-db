import { useAnalytics } from '@/hooks/useStats';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import {
  PieChart,
  Pie,
  Cell,
  LineChart,
  Line,
  AreaChart,
  Area,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from 'recharts';
import { Activity, Gauge, ShieldAlert, BarChart3, TrendingUp, Database, Layers } from 'lucide-react';
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
  const timeSeries10m = data?.time_series_10m;
  const throughputPerMinute = timeSeries10m?.throughput_per_minute ?? [];
  const recentLatencies = timeSeries10m?.recent_latencies ?? [];
  const avgPointsPerSecond = timeSeries10m?.avg_points_per_second ?? 0;
  const cacheHitRatePct = data?.cache_hit_rate_pct ?? null;
  const throughputChartData = throughputPerMinute.map((b) => ({
    time: format(new Date(b.timestamp * 1000), 'HH:mm'),
    timestamp: b.timestamp,
    points: b.points,
  }));
  const latencyAreaData = recentLatencies.map((e) => ({
    time: format(new Date(e.timestamp * 1000), 'HH:mm:ss'),
    timestamp: e.timestamp,
    took_ms: e.took_ms,
  }));

  // Histograma de latência (P95): buckets para distribuição das últimas consultas (10 min)
  const latencyBuckets = [
    { range: '0-5ms', min: 0, max: 5 },
    { range: '5-10ms', min: 5, max: 10 },
    { range: '10-25ms', min: 10, max: 25 },
    { range: '25-50ms', min: 25, max: 50 },
    { range: '50-100ms', min: 50, max: 100 },
    { range: '100ms+', min: 100, max: Infinity },
  ];
  const latencyHistogramData = latencyBuckets.map(({ range, min, max }) => ({
    range,
    count: recentLatencies.filter((e) => {
      const ms = e.took_ms;
      return ms >= min && ms < max;
    }).length,
  }));

  const topNamespacesByStorage = data?.top_namespaces_by_storage ?? [];
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

        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Cache Hit Rate %</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-500/10">
              <Database className="h-4 w-4 text-emerald-500" />
            </div>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : cacheHitRatePct !== null ? (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {typeof cacheHitRatePct === 'number' ? cacheHitRatePct.toFixed(1) : cacheHitRatePct}%
              </p>
            ) : (
              <p className="text-sm text-gray-400">No searches yet</p>
            )}
          </CardContent>
        </Card>

        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Re-ranking Overhead (ms)</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-violet-500/10">
              <BarChart3 className="h-4 w-4 text-violet-500" />
            </div>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : data?.rerank_overhead_ms_avg != null ? (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {Number(data.rerank_overhead_ms_avg).toFixed(2)}
              </p>
            ) : (
              <p className="text-sm text-gray-400">No rerank queries yet</p>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Top Namespaces by Storage */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Layers className="h-5 w-5 text-orange-500" />
            Top Namespaces by Storage
          </CardTitle>
          <p className="text-sm text-gray-400">
            Tenants/namespaces que mais consomem recursos (pontos e armazenamento estimado).
          </p>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <Skeleton className="h-[220px] w-full rounded" />
          ) : topNamespacesByStorage.length > 0 ? (
            <>
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Namespace</TableHead>
                      <TableHead className="text-right">Points</TableHead>
                      <TableHead className="text-right">Storage (est.)</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {topNamespacesByStorage.map((row) => (
                      <TableRow key={row.namespace}>
                        <TableCell className="font-medium">
                          <Badge variant="default" className="font-mono">
                            {row.namespace}
                          </Badge>
                        </TableCell>
                        <TableCell className="text-right tabular-nums">
                          {row.point_count.toLocaleString()}
                        </TableCell>
                        <TableCell className="text-right tabular-nums text-gray-400">
                          {(row.storage_bytes_estimate / 1024).toFixed(1)} KB
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
              <div className="mt-4 h-[200px]">
                <ResponsiveContainer width="100%" height={200}>
                  <BarChart
                    data={topNamespacesByStorage.slice(0, 15).map((r) => ({
                      name: r.namespace.length > 12 ? r.namespace.slice(0, 12) + '…' : r.namespace,
                      points: r.point_count,
                      storage_kb: Math.round(r.storage_bytes_estimate / 1024),
                    }))}
                    layout="vertical"
                    margin={{ top: 4, right: 24, left: 0, bottom: 4 }}
                  >
                    <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                    <XAxis type="number" stroke="#9ca3af" />
                    <YAxis type="category" dataKey="name" width={80} stroke="#9ca3af" />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: '#2d2d2d',
                        border: '1px solid #3f3f3f',
                        borderRadius: '8px',
                      }}
                      formatter={(value: number | undefined, _name: string | undefined, props: { payload?: { storage_kb?: number } }) =>
                        typeof value === 'number' && props?.payload?.storage_kb != null
                          ? [`${value.toLocaleString()} pts · ${props.payload.storage_kb} KB`, 'Storage']
                          : [value ?? 0, 'Points']
                      }
                    />
                    <Bar dataKey="points" fill="#f97316" name="Points" radius={[0, 4, 4, 0]} />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </>
          ) : (
            <p className="py-12 text-center text-sm text-gray-400">
              No namespace data yet. Add points with a <code className="text-gray-300">namespace</code> field to see tenant usage.
            </p>
          )}
        </CardContent>
      </Card>

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

      {/* Real-time: Ingestão (pontos/min) + Latência (área + histograma P95) */}
      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <TrendingUp className="h-5 w-5 text-orange-500" />
              Ingestão — Pontos por minuto (últimos 10 min)
            </CardTitle>
            <p className="text-sm text-gray-400">
              Média: {typeof avgPointsPerSecond === 'number' ? avgPointsPerSecond.toFixed(2) : 0} pts/s
            </p>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-[250px] w-full rounded" />
            ) : throughputChartData.length > 0 ? (
              <ResponsiveContainer width="100%" height={250}>
                <LineChart data={throughputChartData}>
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
                    dataKey="points"
                    stroke="#22c55e"
                    strokeWidth={2}
                    name="Pontos/min"
                    dot={false}
                  />
                </LineChart>
              </ResponsiveContainer>
            ) : (
              <p className="py-12 text-center text-sm text-gray-400">Sem dados de ingestão nos últimos 10 min</p>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Latência de Busca — últimas consultas</CardTitle>
            <p className="text-sm text-gray-400">
              P95 (10 min): {timeSeries10m?.p95_latency_ms != null ? Number(timeSeries10m.p95_latency_ms).toFixed(2) : '—'} ms
            </p>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-[250px] w-full rounded" />
            ) : latencyAreaData.length > 0 ? (
              <ResponsiveContainer width="100%" height={250}>
                <AreaChart data={latencyAreaData}>
                  <defs>
                    <linearGradient id="latencyGradient" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#f97316" stopOpacity={0.4} />
                      <stop offset="95%" stopColor="#f97316" stopOpacity={0} />
                    </linearGradient>
                  </defs>
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
                  <Area
                    type="monotone"
                    dataKey="took_ms"
                    stroke="#f97316"
                    strokeWidth={2}
                    fill="url(#latencyGradient)"
                    name="Latência (ms)"
                  />
                </AreaChart>
              </ResponsiveContainer>
            ) : (
              <p className="py-12 text-center text-sm text-gray-400">Sem consultas recentes</p>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Histograma P95: distribuição de latências (10 min) */}
      <Card>
        <CardHeader>
          <CardTitle>Latência — Histograma P95 (últimos 10 min)</CardTitle>
          <p className="text-sm text-gray-400">
            Distribuição das consultas por faixa de latência; P95 (10 min): {timeSeries10m?.p95_latency_ms != null ? Number(timeSeries10m.p95_latency_ms).toFixed(2) : '—'} ms
          </p>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <Skeleton className="h-[220px] w-full rounded" />
          ) : latencyHistogramData.some((d) => d.count > 0) ? (
            <ResponsiveContainer width="100%" height={220}>
              <BarChart data={latencyHistogramData} margin={{ top: 8, right: 16, left: 0, bottom: 8 }}>
                <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                <XAxis dataKey="range" stroke="#9ca3af" />
                <YAxis stroke="#9ca3af" allowDecimals={false} />
                <Tooltip
                  contentStyle={{
                    backgroundColor: '#2d2d2d',
                    border: '1px solid #3f3f3f',
                    borderRadius: '8px',
                  }}
                  labelStyle={{ color: '#f9fafb' }}
                />
                <Bar dataKey="count" fill="#f97316" name="Consultas" radius={[4, 4, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          ) : (
            <p className="py-12 text-center text-sm text-gray-400">Sem dados de latência nos últimos 10 min</p>
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
