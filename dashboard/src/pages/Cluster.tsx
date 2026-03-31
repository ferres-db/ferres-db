import { useCluster } from '@/hooks/useStats';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { Server, Crown, Users, Copy } from 'lucide-react';
import { cn } from '@/utils/cn';

export const Cluster = () => {
  const { data: cluster, isLoading, error } = useCluster();

  if (error) {
    return (
      <div className="space-y-6">
        <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Cluster</h1>
        <Card className="border-red-500/20 bg-red-500/5">
          <CardContent className="pt-6">
            <p className="text-sm text-red-400">
              Failed to load cluster status: {(error as Error).message}
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Cluster</h1>
        <p className="mt-1 text-sm text-gray-400">
          Raft-based consensus: active nodes, leader, and replication status.
        </p>
      </div>

      {isLoading ? (
        <div className="grid gap-4 md:grid-cols-2">
          <Skeleton className="h-32 rounded-xl" />
          <Skeleton className="h-32 rounded-xl" />
        </div>
      ) : (
        <>
          <div className="grid gap-4 md:grid-cols-2">
            <Card className="border-white/[0.06] bg-bg-secondary">
              <CardHeader className="pb-2">
                <CardTitle className="flex items-center gap-2 text-base font-medium text-gray-200">
                  <Server className="h-4 w-4 text-orange-500" />
                  Consensus
                </CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge
                    variant="default"
                    className={cn(
                      cluster?.raft_enabled && 'bg-emerald-600/80 hover:bg-emerald-600/90',
                    )}
                  >
                    Raft: {cluster?.raft_enabled ? 'Enabled' : 'Standalone'}
                  </Badge>
                  {cluster?.leader_id != null && (
                    <Badge variant="default" className="inline-flex items-center gap-1">
                      <Crown className="h-3.5 w-3.5" />
                      Leader: {cluster.leader_id}
                    </Badge>
                  )}
                </div>
              </CardContent>
            </Card>

            <Card className="border-white/[0.06] bg-bg-secondary">
              <CardHeader className="pb-2">
                <CardTitle className="flex items-center gap-2 text-base font-medium text-gray-200">
                  <Users className="h-4 w-4 text-orange-500" />
                  Nodes
                </CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-2xl font-semibold text-gray-50">
                  {cluster?.nodes?.length ?? 0}
                </p>
                <p className="text-xs text-gray-500">active node(s)</p>
              </CardContent>
            </Card>
          </div>

          <Card className="border-white/[0.06] bg-bg-secondary">
            <CardHeader className="pb-2">
              <CardTitle className="text-base font-medium text-gray-200">
                Node list
              </CardTitle>
            </CardHeader>
            <CardContent>
              {!cluster?.nodes?.length ? (
                <p className="text-sm text-gray-500">No nodes reported.</p>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-white/[0.06] text-left text-gray-400">
                        <th className="pb-2 pr-4 font-medium">ID</th>
                        <th className="pb-2 pr-4 font-medium">Address</th>
                        <th className="pb-2 pr-4 font-medium">Role</th>
                        <th className="pb-2 font-medium">Replication lag</th>
                      </tr>
                    </thead>
                    <tbody>
                      {cluster.nodes.map((node) => (
                        <tr
                          key={node.id}
                          className="border-b border-white/[0.04] text-gray-300"
                        >
                          <td className="py-2 pr-4 font-mono text-gray-200">{node.id}</td>
                          <td className="py-2 pr-4 flex items-center gap-1">
                            <code className="rounded bg-white/5 px-1.5 py-0.5 text-xs">
                              {node.addr}
                            </code>
                            <button
                              type="button"
                              className="rounded p-1 text-gray-500 hover:bg-white/10 hover:text-gray-300"
                              title="Copy address"
                              onClick={() => navigator.clipboard.writeText(node.addr)}
                            >
                              <Copy className="h-3.5 w-3.5" />
                            </button>
                          </td>
                          <td className="py-2 pr-4">
                            <Badge
                              variant="default"
                              className={cn(
                                node.role === 'leader' &&
                                  'bg-amber-600/80 hover:bg-amber-600/90',
                              )}
                            >
                              {node.role === 'leader' && (
                                <Crown className="mr-1 h-3 w-3" />
                              )}
                              {node.role}
                            </Badge>
                          </td>
                          <td className="py-2 text-gray-500">
                            {node.replication_lag != null
                              ? String(node.replication_lag)
                              : '—'}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </CardContent>
          </Card>
        </>
      )}
    </div>
  );
};
