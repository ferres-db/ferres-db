import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useAudit } from '@/hooks/useAudit';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { RefreshCw, Filter } from 'lucide-react';
import { format } from 'date-fns';
import type { AuditQueryParams } from '@/types';

const ACTIONS_OPTIONS = [
  '',
  'search',
  'search_hybrid',
  'upsert',
  'delete_points',
  'create_collection',
  'delete_collection',
  'login',
  'create_user',
  'delete_user',
  'update_password',
  'update_permissions',
  'create_api_key',
  'delete_api_key',
  'save',
];

export const Audit = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role !== 'admin') navigate('/', { replace: true });
  }, [role, navigate]);

  const [params, setParams] = useState<AuditQueryParams>({ limit: 100 });
  const [userFilter, setUserFilter] = useState('');
  const [actionFilter, setActionFilter] = useState('');
  const [resourceFilter, setResourceFilter] = useState('');

  const { data: entries, isLoading, isFetching, refetch } = useAudit({
    ...params,
    user: userFilter.trim() || undefined,
    action: actionFilter.trim() || undefined,
    resource: resourceFilter.trim() || undefined,
  });

  const applyFilters = () => {
    setParams((p) => ({
      ...p,
      user: userFilter.trim() || undefined,
      action: actionFilter.trim() || undefined,
      resource: resourceFilter.trim() || undefined,
    }));
  };

  const resultVariant = (result: string) => {
    if (result === 'success') return 'success';
    if (result === 'denied') return 'warning';
    return 'danger';
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">Audit</h1>
          <p className="text-gray-600 mt-2">Action log for FerresDB (Admin only)</p>
        </div>
        <Button variant="secondary" onClick={() => refetch()} disabled={isFetching}>
          <RefreshCw className={`h-4 w-4 mr-2 ${isFetching ? 'animate-spin' : ''}`} />
          Refresh
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Filter className="h-5 w-5" />
            Filters
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
            <div>
              <label className="mb-1 block text-sm font-medium text-gray-400">User</label>
              <Input
                value={userFilter}
                onChange={(e) => setUserFilter(e.target.value)}
                placeholder="user_id"
              />
            </div>
            <div>
              <label className="mb-1 block text-sm font-medium text-gray-400">Action</label>
              <select
                className="flex h-10 w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50"
                value={actionFilter}
                onChange={(e) => setActionFilter(e.target.value)}
              >
                {ACTIONS_OPTIONS.map((a) => (
                  <option key={a} value={a}>
                    {a || '— All —'}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="mb-1 block text-sm font-medium text-gray-400">Resource (contains)</label>
              <Input
                value={resourceFilter}
                onChange={(e) => setResourceFilter(e.target.value)}
                placeholder="e.g. collection:docs"
              />
            </div>
          </div>
          <div className="mt-3 flex gap-2">
            <Button onClick={applyFilters}>Apply</Button>
            <Button
              variant="secondary"
              onClick={() => {
                setUserFilter('');
                setActionFilter('');
                setResourceFilter('');
                setParams({ limit: 100 });
              }}
            >
              Clear
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Entries</CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Date / Time</TableHead>
                  <TableHead>User</TableHead>
                  <TableHead>Action</TableHead>
                  <TableHead>Resource</TableHead>
                  <TableHead>Result</TableHead>
                  <TableHead>Duration</TableHead>
                  <TableHead>Details</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {Array.isArray(entries) &&
                  entries.map((entry, i) => (
                    <TableRow key={`${entry.timestamp}-${entry.user_id}-${i}`}>
                      <TableCell className="text-sm text-gray-400 whitespace-nowrap">
                        {format(new Date(entry.timestamp), 'dd/MM/yyyy HH:mm:ss')}
                      </TableCell>
                      <TableCell className="font-medium">{entry.user_id}</TableCell>
                      <TableCell>
                        <span className="text-sm">{entry.action}</span>
                      </TableCell>
                      <TableCell className="text-sm text-gray-400 max-w-[180px] truncate" title={entry.resource}>
                        {entry.resource}
                      </TableCell>
                      <TableCell>
                        <Badge variant={resultVariant(entry.result)} className="capitalize">
                          {entry.result}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-sm">
                        {entry.duration_ms != null ? `${entry.duration_ms} ms` : '—'}
                      </TableCell>
                      <TableCell className="text-sm text-gray-400 max-w-[200px] truncate" title={JSON.stringify(entry.details)}>
                        {Object.keys(entry.details).length > 0 ? JSON.stringify(entry.details) : '—'}
                      </TableCell>
                    </TableRow>
                  ))}
                {(!entries || entries.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={7} className="text-center text-gray-500 py-8">
                      No entries found. Adjust filters or wait for new actions.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
};
