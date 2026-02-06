import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useApiKeys, useCreateApiKey, useDeleteApiKey } from '@/hooks/useApiKeys';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Modal } from '@/components/ui/Modal';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Skeleton } from '@/components/ui/Skeleton';
import { Plus, Trash2, Copy, Check } from 'lucide-react';
import { format } from 'date-fns';
import type { CreateApiKeyResponse } from '@/types';

export const ApiKeys = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === 'viewer') navigate('/', { replace: true });
  }, [role, navigate]);
  const { data: keys, isLoading, isError: listErrorFlag, error: listErrorRaw } = useApiKeys();
  const createKey = useCreateApiKey();
  const deleteKey = useDeleteApiKey();
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [newKeyName, setNewKeyName] = useState('');
  const [createdKey, setCreatedKey] = useState<CreateApiKeyResponse | null>(null);
  const [copied, setCopied] = useState(false);

  const handleCreate = async () => {
    const name = newKeyName.trim();
    if (!name) return;
    try {
      const result = await createKey.mutateAsync(name);
      setIsCreateModalOpen(false);
      setNewKeyName('');
      setCreatedKey(result);
    } catch {
      // Error handled by mutation
    }
  };

  const handleCopyKey = async () => {
    if (!createdKey?.key) return;
    await navigator.clipboard.writeText(createdKey.key);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleCloseCreatedModal = () => {
    setCreatedKey(null);
  };

  const handleDelete = async (id: number, name: string) => {
    if (!confirm(`Remove API key "${name}"? This key will stop working immediately.`)) return;
    try {
      await deleteKey.mutateAsync(id);
    } catch {
      // Error handled by mutation
    }
  };

  const listErr = listErrorRaw as { response?: { status?: number; data?: { message?: string } } } | undefined;
  const createErr = createKey.error as { response?: { status?: number; data?: { message?: string } } } | undefined;
  const isForbidden =
    (listErrorFlag && listErr?.response?.status === 403) || (createKey.isError && createErr?.response?.status === 403);
  const forbiddenMessage =
    listErr?.response?.data?.message || createErr?.response?.data?.message || 'Invalid API key (403).';

  return (
    <div className="space-y-6">
      {isForbidden && (
        <div className="rounded-lg border border-amber-500/50 bg-amber-500/10 px-4 py-3 text-sm text-amber-200">
          <strong>403 Forbidden:</strong> {forbiddenMessage}
          <br />
          Configure <code className="bg-bg-tertiary px-1 rounded">VITE_API_KEY</code> in{' '}
          <code className="bg-bg-tertiary px-1 rounded">dashboard/.env</code> with the same value as{' '}
          <code className="bg-bg-tertiary px-1 rounded">FERRESDB_API_KEYS</code> on the server (or a key created here).
        </div>
      )}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">API Keys</h1>
          <p className="text-gray-600 mt-2">Create and manage keys for API access</p>
        </div>
        {(role === 'admin' || role === 'editor') && (
          <Button onClick={() => setIsCreateModalOpen(true)}>
            <Plus className="h-4 w-4 mr-2" />
            New Key
          </Button>
        )}
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Your API Keys</CardTitle>
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
                  <TableHead>Name</TableHead>
                  <TableHead>Key prefix</TableHead>
                  <TableHead>Created</TableHead>
                  <TableHead>Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {Array.isArray(keys) &&
                  keys.map((key) => (
                    <TableRow key={key.id}>
                      <TableCell className="font-medium">{key.name}</TableCell>
                      <TableCell className="font-mono text-sm text-gray-400">{key.key_prefix}…</TableCell>
                      <TableCell>{format(new Date(key.created_at * 1000), 'MMM dd, yyyy HH:mm')}</TableCell>
                      <TableCell>
                        {(role === 'admin' || role === 'editor') && (
                          <Button
                            variant="danger"
                            size="sm"
                            onClick={() => handleDelete(key.id, key.name)}
                            disabled={deleteKey.isPending}
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                {(!keys || !Array.isArray(keys) || keys.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={4} className="text-center text-gray-500 py-8">
                      No API keys yet. Create one to get started.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <Modal
        isOpen={isCreateModalOpen}
        onClose={() => setIsCreateModalOpen(false)}
        title="Create API Key"
      >
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            handleCreate();
          }}
        >
          <div>
            <label className="block text-sm font-medium mb-1">Key name</label>
            <Input
              value={newKeyName}
              onChange={(e) => setNewKeyName(e.target.value)}
              placeholder="e.g. production, staging"
              autoFocus
            />
          </div>
          {createKey.isError && (
            <p className="text-sm text-red-400">
              {(createKey.error as { response?: { data?: { message?: string } } })?.response?.data?.message ||
                (createKey.error instanceof Error ? createKey.error.message : 'Failed to create key')}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setIsCreateModalOpen(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!newKeyName.trim() || createKey.isPending}>
              {createKey.isPending ? 'Creating…' : 'Create'}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        isOpen={!!createdKey}
        onClose={handleCloseCreatedModal}
        title="API key created"
      >
        <div className="space-y-4">
          <p className="text-sm text-amber-200/90">
            Copy this key now. It won’t be shown again.
          </p>
          <div className="flex items-center gap-2 rounded-md bg-bg-tertiary px-3 py-2 font-mono text-sm break-all">
            <span className="text-gray-50">{createdKey?.key}</span>
            <Button
              variant="secondary"
              size="sm"
              onClick={handleCopyKey}
              className="shrink-0"
            >
              {copied ? <Check className="h-4 w-4 text-green-400" /> : <Copy className="h-4 w-4" />}
              {copied ? ' Copied' : ' Copy'}
            </Button>
          </div>
          <p className="text-xs text-gray-500">
            Use it in the <code className="bg-bg-tertiary px-1 rounded">Authorization: Bearer &lt;key&gt;</code> header.
          </p>
          <div className="flex justify-end">
            <Button onClick={handleCloseCreatedModal}>Done</Button>
          </div>
        </div>
      </Modal>
    </div>
  );
};
