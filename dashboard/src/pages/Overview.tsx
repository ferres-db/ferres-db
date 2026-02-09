import { useGlobalStats, useQueryStats } from '@/hooks/useStats';
import { useCollections } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import { Database, TrendingUp, Activity } from 'lucide-react';

export const Overview = () => {
  const { data: stats, isLoading: statsLoading } = useGlobalStats();
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  useQueryStats();

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Overview</h1>
        <p className="mt-1 text-sm text-gray-400">Monitor your FerresDB instance</p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Collections</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <Database className="h-4 w-4 text-orange-500" />
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

        <Card className="overflow-hidden">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Points</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <Activity className="h-4 w-4 text-orange-500" />
            </div>
          </CardHeader>
          <CardContent>
            {statsLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {stats?.total_points ?? 0}
              </p>
            )}
          </CardContent>
        </Card>

        <Card className="overflow-hidden sm:col-span-2 lg:col-span-1">
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Queries/min</CardTitle>
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-orange-500/10">
              <TrendingUp className="h-4 w-4 text-orange-500" />
            </div>
          </CardHeader>
          <CardContent>
            {statsLoading ? (
              <Skeleton className="h-8 w-16 rounded" />
            ) : (
              <p className="text-2xl font-semibold tabular-nums text-gray-50">
                {stats?.queries_per_minute?.reduce((sum, bucket) => sum + bucket.count, 0) ?? 0}
              </p>
            )}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Recent Activity</CardTitle>
        </CardHeader>
        <CardContent>
          {collectionsLoading ? (
            <div className="space-y-3">
              <Skeleton className="h-4 w-full rounded" />
              <Skeleton className="h-4 w-full rounded" />
              <Skeleton className="h-4 w-3/4 rounded" />
            </div>
          ) : (
            <p className="text-sm text-gray-400">
              {collections?.length ?? 0} collection(s) available
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
};
