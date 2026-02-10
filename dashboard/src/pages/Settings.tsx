import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { backupApi, collectionsApi, settingsApi, type CloudSettings } from '@/api/ferresdb';
import type { Collection } from '@/types';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { CloudUpload, Database, Settings as SettingsIcon } from 'lucide-react';

export const Settings = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  const [exporting, setExporting] = useState(false);
  const [exportResult, setExportResult] = useState<{
    key: string;
    bucket: string;
    size_bytes: number;
    region?: string;
  } | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const [cloud, setCloud] = useState<CloudSettings & { secret_access_key?: string }>({});
  const [cloudLoading, setCloudLoading] = useState(true);
  const [cloudSaving, setCloudSaving] = useState(false);
  const [cloudSaveMessage, setCloudSaveMessage] = useState<string | null>(null);
  const [cloudTestLoading, setCloudTestLoading] = useState(false);
  const [cloudTestMessage, setCloudTestMessage] = useState<{ ok: boolean; message: string } | null>(null);

  const [collections, setCollections] = useState<Collection[]>([]);
  const [retentionLoading, setRetentionLoading] = useState(true);
  const [retentionSaving, setRetentionSaving] = useState<string | null>(null);
  const [retentionValues, setRetentionValues] = useState<Record<string, string>>({});
  const [retentionMessage, setRetentionMessage] = useState<string | null>(null);

  useEffect(() => {
    if (role !== 'admin') navigate('/', { replace: true });
  }, [role, navigate]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setCloudLoading(true);
      try {
        const data = await settingsApi.getCloud();
        if (!cancelled) {
          setCloud({
            region: data.region ?? '',
            bucket: data.bucket ?? '',
            endpoint: data.endpoint ?? '',
            access_key_id: data.access_key_id ?? '',
            secret_access_key: '', // never returned by API
          });
        }
      } catch {
        if (!cancelled) setCloud({ region: '', bucket: '', endpoint: '', access_key_id: '', secret_access_key: '' });
      } finally {
        if (!cancelled) setCloudLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setRetentionLoading(true);
      try {
        const list = await collectionsApi.list();
        if (!cancelled) {
          setCollections(list);
          const initial: Record<string, string> = {};
          list.forEach((c) => {
            initial[c.name] = c.retention_days != null ? String(c.retention_days) : '';
          });
          setRetentionValues(initial);
        }
      } catch {
        if (!cancelled) setCollections([]);
      } finally {
        if (!cancelled) setRetentionLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  const handleSaveRetention = async (name: string) => {
    setRetentionMessage(null);
    setRetentionSaving(name);
    try {
      const raw = retentionValues[name]?.trim() ?? '';
      const value = raw === '' ? null : Math.max(0, Math.floor(Number(raw)));
      if (raw !== '' && (Number.isNaN(value) || value < 0)) {
        setRetentionMessage(`Invalid retention value for ${name}`);
        return;
      }
      await collectionsApi.patchRetention(name, value);
      setRetentionMessage(`Retention for "${name}" saved.`);
    } catch (err: unknown) {
      const msg =
        err && typeof err === 'object' && 'response' in err
          ? (err as { response?: { data?: { message?: string } } }).response?.data?.message
          : err instanceof Error
            ? err.message
            : 'Failed to save retention';
      setRetentionMessage(String(msg));
    } finally {
      setRetentionSaving(null);
    }
  };

  const handleSaveCloud = async () => {
    setCloudSaveMessage(null);
    setCloudTestMessage(null);
    setCloudSaving(true);
    try {
      await settingsApi.putCloud({
        region: cloud.region || null,
        bucket: cloud.bucket || null,
        endpoint: cloud.endpoint || null,
        access_key_id: cloud.access_key_id || null,
        secret_access_key: cloud.secret_access_key || null,
      });
      setCloudSaveMessage('Settings saved.');
      setCloud((c) => ({ ...c, secret_access_key: '' }));
    } catch (err: unknown) {
      const msg =
        err && typeof err === 'object' && 'response' in err
          ? (err as { response?: { data?: { message?: string } } }).response?.data?.message
          : err instanceof Error
            ? err.message
            : 'Failed to save settings';
      setCloudSaveMessage(String(msg));
    } finally {
      setCloudSaving(false);
    }
  };

  const handleTestS3 = async () => {
    setCloudTestMessage(null);
    setCloudTestLoading(true);
    try {
      const result = await settingsApi.testS3();
      setCloudTestMessage({ ok: result.ok, message: result.ok ? 'S3 connection successful.' : result.message ?? 'Connection failed.' });
    } catch (err: unknown) {
      const msg =
        err && typeof err === 'object' && 'response' in err
          ? (err as { response?: { data?: { message?: string } } }).response?.data?.message
          : err instanceof Error
            ? err.message
            : 'Test failed';
      setCloudTestMessage({ ok: false, message: String(msg) });
    } finally {
      setCloudTestLoading(false);
    }
  };

  const handleExportToCloud = async () => {
    setExportError(null);
    setExportResult(null);
    setExporting(true);
    try {
      const result = await backupApi.exportToCloud();
      setExportResult({
        key: result.key,
        bucket: result.bucket,
        size_bytes: result.size_bytes,
        region: result.region,
      });
    } catch (err: unknown) {
      const msg =
        err && typeof err === 'object' && 'response' in err
          ? (err as { response?: { data?: { message?: string } } }).response?.data?.message
          : err instanceof Error
            ? err.message
            : 'Export failed';
      setExportError(String(msg));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <SettingsIcon className="h-6 w-6 text-gray-400" />
        <h1 className="text-2xl font-semibold text-white">Settings</h1>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            Cloud backup (S3)
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-sm text-gray-400">
            Configure region, bucket, optional custom endpoint and credentials for S3 backup. Stored in the server database.
            Credentials (Secret Key) are hidden by default. Leave secret key blank to keep the existing value.
          </p>
          {cloudLoading ? (
            <p className="text-sm text-gray-500">Loading…</p>
          ) : (
            <>
              <div className="grid gap-3 max-w-md">
                <div>
                  <label className="block text-sm font-medium text-gray-300 mb-1">Region</label>
                  <Input
                    type="text"
                    value={cloud.region ?? ''}
                    onChange={(e) => setCloud((c) => ({ ...c, region: e.target.value }))}
                    placeholder="e.g. us-east-1"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-300 mb-1">Bucket</label>
                  <Input
                    type="text"
                    value={cloud.bucket ?? ''}
                    onChange={(e) => setCloud((c) => ({ ...c, bucket: e.target.value }))}
                    placeholder="bucket name"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-300 mb-1">Endpoint (optional)</label>
                  <Input
                    type="text"
                    value={cloud.endpoint ?? ''}
                    onChange={(e) => setCloud((c) => ({ ...c, endpoint: e.target.value }))}
                    placeholder="e.g. https://s3.amazonaws.com or http://localhost:9000"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-300 mb-1">Access Key ID</label>
                  <Input
                    type="text"
                    value={cloud.access_key_id ?? ''}
                    onChange={(e) => setCloud((c) => ({ ...c, access_key_id: e.target.value }))}
                    placeholder="AWS access key ID"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-300 mb-1">Secret Access Key</label>
                  <Input
                    type="password"
                    value={cloud.secret_access_key ?? ''}
                    onChange={(e) => setCloud((c) => ({ ...c, secret_access_key: e.target.value }))}
                    placeholder="Leave blank to keep current"
                    autoComplete="off"
                  />
                </div>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button onClick={handleSaveCloud} disabled={cloudSaving} variant="secondary">
                  {cloudSaving ? 'Saving…' : 'Save cloud settings'}
                </Button>
                <Button onClick={handleTestS3} disabled={cloudTestLoading} variant="outline">
                  {cloudTestLoading ? 'Testing…' : 'Test S3 connection'}
                </Button>
              </div>
              {cloudTestMessage && (
                <p className={`text-sm ${cloudTestMessage.ok ? 'text-green-400' : 'text-amber-400'}`}>
                  {cloudTestMessage.message}
                </p>
              )}
              {cloudSaveMessage && (
                <p className={`text-sm ${cloudSaveMessage.startsWith('Settings saved') ? 'text-green-400' : 'text-red-400'}`}>
                  {cloudSaveMessage}
                </p>
              )}
            </>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            <Database className="h-5 w-5" />
            Data retention
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-sm text-gray-400">
            Configure how many days of WAL and snapshot history to keep per collection. The background worker compacts the WAL hourly, removing entries older than the configured period. Leave blank for no limit.
          </p>
          {retentionLoading ? (
            <p className="text-sm text-gray-500">Loading collections…</p>
          ) : collections.length === 0 ? (
            <p className="text-sm text-gray-500">No collections yet.</p>
          ) : (
            <div className="space-y-3 max-w-2xl">
              {collections.map((c) => (
                <div key={c.name} className="flex flex-wrap items-center gap-2">
                  <span className="text-sm font-medium text-gray-300 min-w-[140px]">{c.name}</span>
                  <Input
                    type="number"
                    min={0}
                    placeholder="Days (blank = no limit)"
                    className="w-32"
                    value={retentionValues[c.name] ?? ''}
                    onChange={(e) =>
                      setRetentionValues((prev) => ({ ...prev, [c.name]: e.target.value }))
                    }
                  />
                  <span className="text-xs text-gray-500">days</span>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={retentionSaving !== null}
                    onClick={() => handleSaveRetention(c.name)}
                  >
                    {retentionSaving === c.name ? 'Saving…' : 'Save'}
                  </Button>
                </div>
              ))}
              {retentionMessage && (
                <p
                  className={
                    retentionMessage.startsWith('Retention for')
                      ? 'text-sm text-green-400'
                      : 'text-sm text-amber-400'
                  }
                >
                  {retentionMessage}
                </p>
              )}
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
        <CardTitle className="flex items-center gap-2 text-lg">
            <CloudUpload className="h-5 w-5" />
            Export to cloud
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-sm text-gray-400">
            Creates a binary snapshot of all collections and uploads it to the configured S3 bucket
            (using the region, bucket and credentials above or server config).
          </p>
          <Button
            onClick={handleExportToCloud}
            disabled={exporting}
            variant="secondary"
          >
            {exporting ? 'Exporting…' : 'Export to Cloud'}
          </Button>
          {exportError && (
            <div className="rounded-md bg-red-500/10 border border-red-500/30 px-3 py-2 text-sm text-red-300">
              {exportError}
            </div>
          )}
          {exportResult && (
            <div className="rounded-md bg-green-500/10 border border-green-500/30 px-3 py-2 text-sm text-green-300 space-y-1">
              <p className="font-medium">Backup uploaded successfully</p>
              <p>Bucket: {exportResult.bucket}</p>
              <p>Key: {exportResult.key}</p>
              <p>Size: {(exportResult.size_bytes / 1024).toFixed(1)} KB</p>
              {exportResult.region && <p>Region: {exportResult.region}</p>}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
};
