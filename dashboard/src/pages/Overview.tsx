import { useGlobalStats, useQueryStats } from '@/hooks/useStats';
import { useCollections } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import { Database, TrendingUp, Activity } from 'lucide-react';

export const Overview = () => {
  const { data: stats, isLoading: statsLoading } = useGlobalStats();
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  useQueryStats(); // Keep for future use

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold text-gray-50">Overview</h1>
        <p className="text-gray-400 mt-2">Monitor your FerresDB instance</p>
      </div>

      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-300">Total Collections</CardTitle>
            <Database className="h-4 w-4 text-gray-400" />
          </CardHeader>
          <CardContent>
            {statsLoading ? (
              <Skeleton className="h-8 w-20" />
            ) : (
              <div className="text-2xl font-bold text-gray-50">{stats?.total_collections || collections?.length || 0}</div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-300">Total Points</CardTitle>
            <Activity className="h-4 w-4 text-gray-400" />
          </CardHeader>
          <CardContent>
            {statsLoading ? (
              <Skeleton className="h-8 w-20" />
            ) : (
              <div className="text-2xl font-bold text-gray-50">{stats?.total_points || 0}</div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-gray-300">Queries/min</CardTitle>
            <TrendingUp className="h-4 w-4 text-gray-400" />
          </CardHeader>
          <CardContent>
            {statsLoading ? (
              <Skeleton className="h-8 w-20" />
            ) : (
              <div className="text-2xl font-bold text-gray-50">
                {stats?.queries_per_minute?.reduce((sum, bucket) => sum + bucket.count, 0) || 0}
              </div>
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
            <div className="space-y-2">
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
            </div>
          ) : (
            <p className="text-sm text-gray-400">
              {collections?.length || 0} collection(s) available
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
};
