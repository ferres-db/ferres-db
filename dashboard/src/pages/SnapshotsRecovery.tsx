import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import {
  restoreApi,
  type CollectionRestorePoints,
} from '@/api/ferresdb';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/Table';
import { Badge } from '@/components/ui/Badge';
import { Modal } from '@/components/ui/Modal';
import { History, RotateCcw, AlertCircle, AlertTriangle } from 'lucide-react';

function formatTs(ts: number): string {
  if (!ts) return '—';
  try {
    return new Date(ts * 1000).toISOString().replace('T', ' ').slice(0, 19);
  } catch {
    return String(ts);
  }
}

export const SnapshotsRecovery = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  const [points, setPoints] = useState<Record<string, CollectionRestorePoints>>({});
  const [loading, setLoading] = useState(true);
  const [restoring, setRestoring] = useState(false);
  const [restoreTs, setRestoreTs] = useState('');
  const [restoreDateTime, setRestoreDateTime] = useState('');
  const [restoreCollection, setRestoreCollection] = useState<string>('');
  const [showRestoreConfirm, setShowRestoreConfirm] = useState(false);
  const [restoreResult, setRestoreResult] = useState<{
    ok: boolean;
    restored: string[];
    errors: string[];
  } | null>(null);

  useEffect(() => {
    if (role !== 'admin') navigate('/', { replace: true });
  }, [role, navigate]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      try {
        const data = await restoreApi.getRestorePoints();
        if (!cancelled) setPoints(data);
      } catch {
        if (!cancelled) setPoints({});
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const getTimestampFromInput = (): number | null => {
    if (restoreDateTime) {
      const ms = new Date(restoreDateTime).getTime();
      if (!Number.isNaN(ms)) return Math.floor(ms / 1000);
    }
    const ts = parseInt(restoreTs, 10);
    if (!Number.isNaN(ts) && ts > 0) return ts;
    return null;
  };

  const handleRestoreClick = () => {
    const ts = getTimestampFromInput();
    if (ts == null) {
      setRestoreResult({
        ok: false,
        restored: [],
        errors: ['Select a date/time or enter a Unix timestamp (seconds).'],
      });
      return;
    }
    setRestoreTs(String(ts));
    setShowRestoreConfirm(true);
  };

  const handleRestoreConfirm = async () => {
    const ts = getTimestampFromInput();
    if (ts == null) {
      setShowRestoreConfirm(false);
      return;
    }
    setRestoring(true);
    setRestoreResult(null);
    setShowRestoreConfirm(false);
    try {
      const result = await restoreApi.restoreToTimestamp(
        ts,
        restoreCollection || undefined
      );
      setRestoreResult(result);
      if (result.restored.length > 0) {
        const data = await restoreApi.getRestorePoints();
        setPoints(data);
      }
    } catch (err: unknown) {
      const msg =
        err && typeof err === 'object' && 'response' in err
          ? (err as { response?: { data?: { message?: string } } }).response?.data
              ?.message
          : err instanceof Error
            ? err.message
            : 'Restore failed';
      setRestoreResult({
        ok: false,
        restored: [],
        errors: [String(msg)],
      });
    } finally {
      setRestoring(false);
    }
  };

  const collectionNames = Object.keys(points).sort();
  const allTimestamps = new Set<number>();
  Object.values(points).forEach((p) => {
    if (p.last_snapshot_timestamp) allTimestamps.add(p.last_snapshot_timestamp);
    p.wal_timestamps.forEach((t) => allTimestamps.add(t));
  });
  const sortedTimestamps = Array.from(allTimestamps).sort((a, b) => a - b);

  return (
    <div className="space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold text-white flex items-center gap-2">
          <History className="h-7 w-7 opacity-90" />
          Snapshots & Recovery
        </h1>
        <p className="text-gray-400 mt-1 text-sm">
          Point-in-Time Recovery (PITR): view restore points and restore collections to a previous timestamp using the WAL.
        </p>
      </div>

      <Card className="border-white/[0.08] bg-bg-secondary">
        <CardHeader>
          <CardTitle className="text-lg text-white">Restore points</CardTitle>
          <p className="text-sm text-gray-400">
            Last snapshot timestamp and WAL entry timestamps per collection. Use a timestamp below to restore.
          </p>
        </CardHeader>
        <CardContent>
          {loading ? (
            <p className="text-gray-400">Loading…</p>
          ) : collectionNames.length === 0 ? (
            <p className="text-gray-400">No collections with restore data.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow className="border-white/[0.08] hover:bg-transparent">
                  <TableHead className="text-gray-400">Collection</TableHead>
                  <TableHead className="text-gray-400">Last snapshot</TableHead>
                  <TableHead className="text-gray-400">WAL timestamps (count)</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {collectionNames.map((name) => {
                  const p = points[name];
                  return (
                    <TableRow
                      key={name}
                      className="border-white/[0.08] hover:bg-white/[0.04]"
                    >
                      <TableCell className="font-medium text-white">
                        {name}
                      </TableCell>
                      <TableCell className="text-gray-300">
                        {formatTs(p.last_snapshot_timestamp)}
                        {p.last_snapshot_timestamp ? (
                          <Badge variant="default" className="ml-2 text-xs">
                            {p.last_snapshot_timestamp}
                          </Badge>
                        ) : null}
                      </TableCell>
                      <TableCell className="text-gray-300">
                        {p.wal_timestamps.length === 0
                          ? '—'
                          : `${p.wal_timestamps.length} point(s)`}
                        {p.wal_timestamps.length > 0 && (
                          <span className="text-gray-500 text-xs block mt-1">
                            Latest: {formatTs(p.wal_timestamps[p.wal_timestamps.length - 1])}
                          </span>
                        )}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <Card className="border-white/[0.08] bg-bg-secondary">
        <CardHeader>
          <CardTitle className="text-lg text-white flex items-center gap-2">
            <RotateCcw className="h-5 w-5 opacity-90" />
            Point-in-Time Restore
          </CardTitle>
          <p className="text-sm text-gray-400">
            Restore one collection or all collections to the state at a given date/time. The server loads the last snapshot and reapplies WAL entries up to that time. This operation resets database state — use with caution.
          </p>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-wrap items-end gap-4">
            <div className="flex flex-col gap-1">
              <label className="text-sm text-gray-400">Date &amp; time</label>
              <input
                type="datetime-local"
                value={restoreDateTime}
                onChange={(e) => {
                  setRestoreDateTime(e.target.value);
                  const ms = new Date(e.target.value).getTime();
                  if (!Number.isNaN(ms)) setRestoreTs(String(Math.floor(ms / 1000)));
                }}
                className="max-w-[220px] rounded-md border border-white/[0.1] bg-white/[0.06] px-3 py-2 text-sm text-white [color-scheme:dark]"
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-sm text-gray-400">Or Unix timestamp (seconds)</label>
              <Input
                type="number"
                placeholder="e.g. 1739182800"
                value={restoreTs}
                onChange={(e) => setRestoreTs(e.target.value)}
                className="max-w-[200px] bg-white/[0.06] border-white/[0.1] text-white"
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-sm text-gray-400">Collection (optional)</label>
              <select
                value={restoreCollection}
                onChange={(e) => setRestoreCollection(e.target.value)}
                className="rounded-md border border-white/[0.1] bg-white/[0.06] px-3 py-2 text-sm text-white max-w-[220px]"
              >
                <option value="">All collections</option>
                {collectionNames.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </div>
            <Button
              onClick={handleRestoreClick}
              disabled={restoring}
              className="bg-amber-600 hover:bg-amber-700 text-white"
            >
              {restoring ? 'Restoring…' : 'Point-in-Time Restore'}
            </Button>
          </div>

          <Modal
            isOpen={showRestoreConfirm}
            onClose={() => setShowRestoreConfirm(false)}
            title="Confirm Point-in-Time Restore"
          >
            <div className="space-y-4">
              <div className="flex gap-3 p-3 rounded-lg bg-amber-500/10 border border-amber-500/30">
                <AlertTriangle className="h-6 w-6 shrink-0 text-amber-400" />
                <div className="text-sm text-gray-200">
                  <p className="font-medium text-amber-200">This operation will reset database state.</p>
                  <p className="mt-1 text-gray-400">
                    The server will load the last snapshot and replay the WAL up to the selected timestamp. All data after that point will be discarded. Make sure you have backups if needed.
                  </p>
                  <p className="mt-2 text-gray-300">
                    Target: <strong>{restoreTs ? new Date(parseInt(restoreTs, 10) * 1000).toISOString() : '—'}</strong>
                    {restoreCollection ? ` · Collection: ${restoreCollection}` : ' · All collections'}
                  </p>
                </div>
              </div>
              <div className="flex justify-end gap-2">
                <Button variant="secondary" onClick={() => setShowRestoreConfirm(false)}>
                  Cancel
                </Button>
                <Button
                  className="bg-amber-600 hover:bg-amber-700 text-white"
                  onClick={handleRestoreConfirm}
                  disabled={restoring}
                >
                  {restoring ? 'Restoring…' : 'Yes, restore'}
                </Button>
              </div>
            </div>
          </Modal>
          {sortedTimestamps.length > 0 && (
            <p className="text-xs text-gray-500">
              Available timestamps (from snapshot + WAL):{' '}
              {sortedTimestamps.slice(-10).map((t) => {
                const d = new Date(t * 1000);
                const localStr = Number.isNaN(d.getTime())
                  ? ''
                  : `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}T${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
                return (
                  <button
                    key={t}
                    type="button"
                    onClick={() => {
                      setRestoreTs(String(t));
                      setRestoreDateTime(localStr);
                    }}
                    className="mr-2 underline hover:text-gray-300"
                  >
                    {t}
                  </button>
                );
              })}
              {sortedTimestamps.length > 10 && ' …'}
            </p>
          )}
          {restoreResult && (
            <div
              className={
                restoreResult.ok
                  ? 'rounded-md bg-green-500/10 border border-green-500/30 p-3 text-sm text-green-200'
                  : 'rounded-md bg-red-500/10 border border-red-500/30 p-3 text-sm text-red-200'
              }
            >
              {restoreResult.ok ? (
                <>
                  Restored: {restoreResult.restored.join(', ') || '—'}
                  {restoreResult.errors.length > 0 && (
                    <span className="block mt-1">
                      Warnings: {restoreResult.errors.join('; ')}
                    </span>
                  )}
                </>
              ) : (
                <div className="flex items-start gap-2">
                  <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                  <div>
                    {restoreResult.errors.map((e, i) => (
                      <p key={i}>{e}</p>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
};
