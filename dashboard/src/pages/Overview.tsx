import { Link } from 'react-router-dom';
import { useGlobalStats, useQueryStats } from '@/hooks/useStats';
import { useCollections } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import {
  Database,
  TrendingUp,
  Activity,
  Clock,
  Sparkles,
  BarChart3,
  ChevronRight,
  Layers,
  Zap,
  Cpu,
  Shield,
} from 'lucide-react';
import { Badge } from '@/components/ui/Badge';
import { formatDistanceToNow } from 'date-fns';
import { cn } from '@/utils/cn';

const linkButtonClass =
  'inline-flex h-8 items-center justify-center gap-2 rounded-lg px-3 text-xs font-medium transition-all duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-orange-500 focus-visible:ring-offset-2 focus-visible:ring-offset-bg-primary';

export const Overview = () => {
  const { data: stats, isLoading: statsLoading } = useGlobalStats();
  const { data: queries, isLoading: queriesLoading } = useQueryStats();
  const { data: collections, isLoading: collectionsLoading } = useCollections();

  const recentQueries = (queries ?? []).slice(0, 8);
  const displayCollections = (collections ?? []).slice(0, 5);
  const totalPoints = stats?.total_points ?? 0;
  const queries24h = stats?.total_queries_24h ?? 0;
  const avgLatencyMs = stats?.avg_latency_ms ?? 0;

  return (
    <div className="space-y-10">
      {/* Hero */}
      <div className="relative overflow-hidden rounded-2xl border border-white/[0.06] bg-gradient-to-br from-orange-500/8 via-transparent to-amber-500/5 px-6 py-8 sm:px-8 sm:py-10">
        <div className="relative z-10">
          <h1 className="text-2xl font-semibold tracking-tight text-gray-50 sm:text-3xl">
            Overview
          </h1>
          <p className="mt-2 max-w-xl text-sm text-gray-400 sm:text-base">
            Monitor collections, metrics, and recent activity for your FerresDB instance.
          </p>
          {!statsLoading && (
            <div className="mt-4 flex flex-wrap items-center gap-2">
              <Badge
                variant={stats?.role === 'replica' ? 'secondary' : 'default'}
                className="inline-flex items-center gap-1.5"
              >
                {stats?.role === 'replica' ? 'Role: Replica' : 'Role: Leader'}
              </Badge>
              <Badge
                variant={stats?.simd_enabled ? 'success' : 'warning'}
                className="inline-flex items-center gap-1.5"
              >
                <Cpu className="h-3.5 w-3.5" />
                {stats?.simd_enabled ? 'SIMD: Active' : 'SIMD: Scalar'}
              </Badge>
              {stats?.namespace_physical_isolation && (
                <Badge variant="secondary" className="inline-flex items-center gap-1.5">
                  <Shield className="h-3.5 w-3.5" />
                  Namespace isolation: On
                </Badge>
              )}
            </div>
          )}
          <div className="mt-6 flex flex-wrap gap-3">
            <Link
              to="/collections"
              className={cn(linkButtonClass, 'bg-orange-500 text-white hover:bg-orange-600')}
            >
              <Database className="h-4 w-4" />
              Collections
            </Link>
            <Link
              to="/query-tester"
              className={cn(linkButtonClass, 'border border-white/20 text-gray-200 hover:bg-white/10')}
            >
              <Sparkles className="h-4 w-4" />
              Query Tester
            </Link>
            <Link
              to="/analytics"
              className={cn(linkButtonClass, 'border border-white/20 text-gray-200 hover:bg-white/10')}
            >
              <BarChart3 className="h-4 w-4" />
              Analytics
            </Link>
          </div>
        </div>
      </div>

      {/* Stats */}
      <div>
        <h2 className="mb-4 text-sm font-medium uppercase tracking-wider text-gray-500">
          Metrics
        </h2>
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <Card className="overflow-hidden border-white/[0.06] transition-colors hover:border-orange-500/20">
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium text-gray-400">Collections</CardTitle>
              <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
                <Layers className="h-4 w-4 text-orange-500" />
              </div>
            </CardHeader>
            <CardContent>
              {statsLoading ? (
                <Skeleton className="h-8 w-16 rounded" />
              ) : (
                <p className="text-2xl font-semibold tabular-nums text-gray-50">
                  {stats?.total_collections ?? collections?.length ?? 0}
                </p>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden border-white/[0.06] transition-colors hover:border-orange-500/20">
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium text-gray-400">Points</CardTitle>
              <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-amber-500/10">
                <Activity className="h-4 w-4 text-amber-500" />
              </div>
            </CardHeader>
            <CardContent>
              {statsLoading ? (
                <Skeleton className="h-8 w-16 rounded" />
              ) : (
                <p className="text-2xl font-semibold tabular-nums text-gray-50">
                  {totalPoints.toLocaleString()}
                </p>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden border-white/[0.06] transition-colors hover:border-orange-500/20">
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium text-gray-400">Queries (24h)</CardTitle>
              <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-500/10">
                <TrendingUp className="h-4 w-4 text-emerald-500" />
              </div>
            </CardHeader>
            <CardContent>
              {statsLoading ? (
                <Skeleton className="h-8 w-16 rounded" />
              ) : (
                <p className="text-2xl font-semibold tabular-nums text-gray-50">
                  {queries24h.toLocaleString()}
                </p>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden border-white/[0.06] transition-colors hover:border-orange-500/20">
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium text-gray-400">Avg latency</CardTitle>
              <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-blue-500/10">
                <Clock className="h-4 w-4 text-blue-500" />
              </div>
            </CardHeader>
            <CardContent>
              {statsLoading ? (
                <Skeleton className="h-8 w-16 rounded" />
              ) : (
                <p className="text-2xl font-semibold tabular-nums text-gray-50">
                  {avgLatencyMs < 1 && avgLatencyMs > 0
                    ? '<1'
                    : Math.round(avgLatencyMs)}
                  <span className="ml-0.5 text-sm font-normal text-gray-400">ms</span>
                </p>
              )}
            </CardContent>
          </Card>
        </div>
      </div>

      {/* Recent queries + Collections */}
      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle className="flex items-center gap-2 text-base">
              <Zap className="h-4 w-4 text-orange-500" />
              Recent queries
            </CardTitle>
            <Link
              to="/query-tester"
              className="inline-flex h-8 items-center gap-1 rounded-lg px-2 text-sm text-gray-400 hover:bg-white/[0.06] hover:text-gray-200"
            >
              Open
              <ChevronRight className="h-4 w-4" />
            </Link>
          </CardHeader>
          <CardContent>
            {queriesLoading ? (
              <div className="space-y-3">
                {[1, 2, 3, 4].map((i) => (
                  <Skeleton key={i} className="h-12 w-full rounded-lg" />
                ))}
              </div>
            ) : recentQueries.length === 0 ? (
              <p className="rounded-lg border border-dashed border-white/10 bg-white/[0.02] py-8 text-center text-sm text-gray-500">
                No recent queries. Use the Query Tester to get started.
              </p>
            ) : (
              <ul className="space-y-2">
                {recentQueries.map((q) => (
                  <li
                    key={q.query_id}
                    className="flex items-center justify-between gap-3 rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2 text-sm transition-colors hover:bg-white/[0.04]"
                  >
                    <div className="min-w-0 flex-1">
                      <span className="font-medium text-gray-200">{q.collection}</span>
                      <span className="ml-2 text-gray-500">limit {q.limit}</span>
                    </div>
                    <div className="flex shrink-0 items-center gap-3">
                      <span
                        className={cn(
                          'tabular-nums',
                          q.took_ms > 100 ? 'text-amber-400' : 'text-gray-400'
                        )}
                      >
                        {Math.round(q.took_ms)} ms
                      </span>
                      <span className="text-xs text-gray-500">
                        {formatDistanceToNow(new Date(q.timestamp), { addSuffix: true })}
                      </span>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle className="flex items-center gap-2 text-base">
              <Database className="h-4 w-4 text-orange-500" />
              Your collections
            </CardTitle>
            <Link
              to="/collections"
              className="inline-flex h-8 items-center gap-1 rounded-lg px-2 text-sm text-gray-400 hover:bg-white/[0.06] hover:text-gray-200"
            >
              View all
              <ChevronRight className="h-4 w-4" />
            </Link>
          </CardHeader>
          <CardContent>
            {collectionsLoading ? (
              <div className="space-y-3">
                {[1, 2, 3, 4].map((i) => (
                  <Skeleton key={i} className="h-12 w-full rounded-lg" />
                ))}
              </div>
            ) : displayCollections.length === 0 ? (
              <p className="rounded-lg border border-dashed border-white/10 bg-white/[0.02] py-8 text-center text-sm text-gray-500">
                No collections yet. Create one in Collections.
              </p>
            ) : (
              <ul className="space-y-2">
                {displayCollections.map((c) => (
                  <Link
                    key={c.name}
                    to={`/collections/${encodeURIComponent(c.name)}`}
                    className="flex items-center justify-between gap-3 rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2 text-sm transition-colors hover:bg-white/[0.06]"
                  >
                    <span className="font-medium text-gray-200">{c.name}</span>
                    <span className="tabular-nums text-gray-500">
                      {(c.num_points ?? c.point_count ?? 0).toLocaleString()} pts
                    </span>
                  </Link>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
};
