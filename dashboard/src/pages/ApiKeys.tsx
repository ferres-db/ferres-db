import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useApiKeys, useCreateApiKey, useDeleteApiKey, useUpdateKeyNamespaces } from '@/hooks/useApiKeys';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Modal } from '@/components/ui/Modal';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Skeleton } from '@/components/ui/Skeleton';
import { Plus, Trash2, Copy, Check, Pencil } from 'lucide-react';
import { format } from 'date-fns';
import type { ApiKeyInfo, CreateApiKeyResponse } from '@/types';

export const ApiKeys = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === 'viewer') navigate('/', { replace: true });
  }, [role, navigate]);
  const { data: keys, isLoading, isError: listErrorFlag, error: listErrorRaw } = useApiKeys();
  const createKey = useCreateApiKey();
  const deleteKey = useDeleteApiKey();
  const updateNamespaces = useUpdateKeyNamespaces();
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [newKeyName, setNewKeyName] = useState('');
  const [newKeyNamespaces, setNewKeyNamespaces] = useState('');
  const [createdKey, setCreatedKey] = useState<CreateApiKeyResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const [editKey, setEditKey] = useState<ApiKeyInfo | null>(null);
  const [editNamespacesValue, setEditNamespacesValue] = useState('');

  const parseNamespaces = (s: string): string[] =>
    s
      .split(',')
      .map((n) => n.trim())
      .filter(Boolean);

  const handleCreate = async () => {
    const name = newKeyName.trim();
    if (!name) return;
    const allowed_namespaces = parseNamespaces(newKeyNamespaces);
    try {
      const result = await createKey.mutateAsync({
        name,
        allowed_namespaces: allowed_namespaces.length > 0 ? allowed_namespaces : undefined,
      });
      setIsCreateModalOpen(false);
      setNewKeyName('');
      setNewKeyNamespaces('');
      setCreatedKey(result);
    } catch {
      // Error handled by mutation
    }
  };

  const openEditNamespaces = (key: ApiKeyInfo) => {
    setEditKey(key);
    setEditNamespacesValue(
      (key.allowed_namespaces && key.allowed_namespaces.length > 0 ? key.allowed_namespaces : []).join(', '),
    );
  };

  const handleUpdateNamespaces = async () => {
    if (editKey == null) return;
    const list = parseNamespaces(editNamespacesValue);
    try {
      await updateNamespaces.mutateAsync({
        id: editKey.id,
        allowed_namespaces: list.length > 0 ? list : null,
      });
      setEditKey(null);
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
          <h1 className="text-2xl font-semibold tracking-tight text-gray-50">API Keys</h1>
          <p className="mt-1 text-sm text-gray-400">Create and manage keys for API access</p>
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
                  <TableHead>Namespaces</TableHead>
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
                      <TableCell className="text-gray-400 text-sm">
                        {key.allowed_namespaces && key.allowed_namespaces.length > 0
                          ? key.allowed_namespaces.join(', ')
                          : 'All'}
                      </TableCell>
                      <TableCell>{format(new Date(key.created_at * 1000), 'MMM dd, yyyy HH:mm')}</TableCell>
                      <TableCell className="flex items-center gap-1">
                        {(role === 'admin' || role === 'editor') && (
                          <>
                            <Button
                              variant="secondary"
                              size="sm"
                              onClick={() => openEditNamespaces(key)}
                              disabled={updateNamespaces.isPending}
                              title="Edit namespaces"
                            >
                              <Pencil className="h-4 w-4" />
                            </Button>
                            <Button
                              variant="danger"
                              size="sm"
                              onClick={() => handleDelete(key.id, key.name)}
                              disabled={deleteKey.isPending}
                            >
                              <Trash2 className="h-4 w-4" />
                            </Button>
                          </>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                {(!keys || !Array.isArray(keys) || keys.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={5} className="text-center text-gray-500 py-8">
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
          <div>
            <label className="block text-sm font-medium mb-1">Allowed namespaces (optional)</label>
            <Input
              value={newKeyNamespaces}
              onChange={(e) => setNewKeyNamespaces(e.target.value)}
              placeholder="e.g. tenant-a, tenant-b (leave empty for all)"
            />
            <p className="text-xs text-gray-500 mt-1">
              Restrict this key to specific namespaces. Empty = access to all namespaces.
            </p>
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

      <Modal
        isOpen={editKey != null}
        onClose={() => setEditKey(null)}
        title="Edit namespaces"
      >
        <div className="space-y-4">
          <p className="text-sm text-gray-400">
            Key: <strong className="text-gray-200">{editKey?.name}</strong>. Restrict access to specific namespaces or leave empty for all.
          </p>
          <div>
            <label className="block text-sm font-medium mb-1">Allowed namespaces</label>
            <Input
              value={editNamespacesValue}
              onChange={(e) => setEditNamespacesValue(e.target.value)}
              placeholder="e.g. tenant-a, tenant-b (empty = all)"
            />
          </div>
          {updateNamespaces.isError && (
            <p className="text-sm text-red-400">
              {(updateNamespaces.error as { response?: { data?: { message?: string } } })?.response?.data?.message ||
                (updateNamespaces.error instanceof Error ? updateNamespaces.error.message : 'Failed to update')}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setEditKey(null)}>
              Cancel
            </Button>
            <Button
              onClick={handleUpdateNamespaces}
              disabled={updateNamespaces.isPending}
            >
              {updateNamespaces.isPending ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
};
