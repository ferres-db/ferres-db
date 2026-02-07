import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useUsers, useCreateUser, useDeleteUser, useUpdateUserPassword, useUpdateUserPermissions } from '@/hooks/useUsers';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Modal } from '@/components/ui/Modal';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Skeleton } from '@/components/ui/Skeleton';
import { PermissionEditor } from '@/components/PermissionEditor';
import { Plus, Trash2, Key, Shield } from 'lucide-react';
import { format } from 'date-fns';
import type { UserInfo, Permission } from '@/types';

const ROLES = ['viewer', 'editor', 'admin'] as const;

export const Users = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role !== 'admin') navigate('/', { replace: true });
  }, [role, navigate]);
  const { data: users, isLoading } = useUsers();
  const createUser = useCreateUser();
  const deleteUser = useDeleteUser();
  const updatePassword = useUpdateUserPassword();
  const updatePermissions = useUpdateUserPermissions();
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [newUsername, setNewUsername] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [newUserRole, setNewUserRole] = useState<string>('viewer');
  const [newUserPermissions, setNewUserPermissions] = useState<Permission[]>([]);
  const [confirmPassword, setConfirmPassword] = useState('');
  const [passwordModal, setPasswordModal] = useState<{ user: UserInfo; newPassword: string } | null>(null);
  const [permissionsModal, setPermissionsModal] = useState<{ user: UserInfo; permissions: Permission[] } | null>(null);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    const username = newUsername.trim();
    if (!username) return;
    if (!newPassword) {
      createUser.reset();
      return;
    }
    if (newPassword !== confirmPassword) return;
    try {
      await createUser.mutateAsync({
        username,
        password: newPassword,
        role: newUserRole || undefined,
        permissions: newUserPermissions.length > 0 ? newUserPermissions : undefined,
      });
      setIsCreateModalOpen(false);
      setNewUsername('');
      setNewPassword('');
      setNewUserRole('viewer');
      setNewUserPermissions([]);
      setConfirmPassword('');
    } catch {
      // Error shown in modal
    }
  };

  const handleDelete = async (u: UserInfo) => {
    if (!confirm(`Remove user "${u.username}"?`)) return;
    try {
      await deleteUser.mutateAsync(u.id);
    } catch {
      // Error handled by mutation
    }
  };

  const handleChangePassword = async (e: React.FormEvent) => {
    if (!passwordModal || !passwordModal.newPassword.trim()) return;
    e.preventDefault();
    try {
      await updatePassword.mutateAsync({
        username: passwordModal.user.username,
        password: passwordModal.newPassword,
      });
      setPasswordModal(null);
    } catch {
      // Error handled by mutation
    }
  };

  const handleSavePermissions = async (e: React.FormEvent) => {
    if (!permissionsModal) return;
    e.preventDefault();
    try {
      await updatePermissions.mutateAsync({
        username: permissionsModal.user.username,
        permissions: permissionsModal.permissions,
      });
      setPermissionsModal(null);
    } catch {
      // Error handled by mutation
    }
  };

  const permissionsSummary = (user: UserInfo) => {
    const perms = user.permissions;
    if (!perms || perms.length === 0) return <span className="text-gray-500 text-sm">Role only</span>;
    return (
      <span className="text-gray-300 text-sm">
        {perms.length} {perms.length === 1 ? 'permission' : 'permissions'}
      </span>
    );
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">Users</h1>
          <p className="text-gray-600 mt-2">Users who can access the dashboard</p>
        </div>
        <Button onClick={() => setIsCreateModalOpen(true)}>
          <Plus className="h-4 w-4 mr-2" />
          New user
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Dashboard users</CardTitle>
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
                  <TableHead>User</TableHead>
                  <TableHead>Role</TableHead>
                  <TableHead>Permissions</TableHead>
                  <TableHead>Created</TableHead>
                  <TableHead>Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {Array.isArray(users) &&
                  users.map((user) => (
                    <TableRow key={user.id}>
                      <TableCell className="font-medium">{user.username}</TableCell>
                      <TableCell>
                        <span className="rounded bg-bg-tertiary px-2 py-0.5 text-xs capitalize">
                          {user.role || 'viewer'}
                        </span>
                      </TableCell>
                      <TableCell>{permissionsSummary(user)}</TableCell>
                      <TableCell>{format(new Date(user.created_at * 1000), 'dd/MM/yyyy HH:mm')}</TableCell>
                      <TableCell>
                        <div className="flex gap-2">
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() =>
                              setPermissionsModal({
                                user,
                                permissions: user.permissions && user.permissions.length > 0 ? [...user.permissions] : [],
                              })
                            }
                            title="Edit granular permissions"
                          >
                            <Shield className="h-4 w-4" />
                          </Button>
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() => setPasswordModal({ user, newPassword: '' })}
                            title="Change password"
                          >
                            <Key className="h-4 w-4" />
                          </Button>
                          <Button
                            variant="danger"
                            size="sm"
                            onClick={() => handleDelete(user)}
                            disabled={deleteUser.isPending}
                            title="Remove user"
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                {(!users || !Array.isArray(users) || users.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={5} className="text-center text-gray-500 py-8">
                      No users besides the default. Create a new user.
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
        title="New user"
      >
        <form onSubmit={handleCreate} className="space-y-4">
          <div>
            <label className="mb-1 block text-sm font-medium">Username</label>
            <Input
              value={newUsername}
              onChange={(e) => setNewUsername(e.target.value)}
              placeholder="username"
              autoComplete="username"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium">Role</label>
            <select
              className="flex h-10 w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-orange-500 focus-visible:ring-offset-2"
              value={newUserRole}
              onChange={(e) => setNewUserRole(e.target.value)}
            >
              {ROLES.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium">Password</label>
            <Input
              type="password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              placeholder="••••••••"
              autoComplete="new-password"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium">Confirm password</label>
            <Input
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              placeholder="••••••••"
              autoComplete="new-password"
            />
          </div>
          <div>
            <PermissionEditor value={newUserPermissions} onChange={setNewUserPermissions} />
          </div>
          {newPassword && confirmPassword && newPassword !== confirmPassword && (
            <p className="text-sm text-red-400">Passwords do not match.</p>
          )}
          {createUser.isError && (
            <p className="text-sm text-red-400">
              {(createUser.error as { response?: { data?: { message?: string } } })?.response?.data?.message ||
                (createUser.error instanceof Error ? createUser.error.message : 'Failed to create user')}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setIsCreateModalOpen(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={
                !newUsername.trim() ||
                !newPassword ||
                newPassword !== confirmPassword ||
                createUser.isPending
              }
            >
              {createUser.isPending ? 'Creating…' : 'Create'}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        isOpen={!!permissionsModal}
        onClose={() => setPermissionsModal(null)}
        title={`Granular permissions: ${permissionsModal?.user.username ?? ''}`}
      >
        <form onSubmit={handleSavePermissions} className="space-y-4 max-h-[70vh] overflow-y-auto">
          <p className="text-sm text-gray-400">
            If empty, access uses the role only. Add permissions to restrict by collection or apply
            metadata restriction (e.g. department=sales).
          </p>
          {permissionsModal && (
            <PermissionEditor
              value={permissionsModal.permissions}
              onChange={(p) => setPermissionsModal({ ...permissionsModal, permissions: p })}
            />
          )}
          {updatePermissions.isError && (
            <p className="text-sm text-red-400">
              {(updatePermissions.error as { response?: { data?: { message?: string } } })?.response?.data
                ?.message ?? 'Failed to save permissions'}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setPermissionsModal(null)}>
              Cancel
            </Button>
            <Button type="submit" disabled={updatePermissions.isPending}>
              {updatePermissions.isPending ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        isOpen={!!passwordModal}
        onClose={() => setPasswordModal(null)}
        title={`Change password: ${passwordModal?.user.username ?? ''}`}
      >
        <form onSubmit={handleChangePassword} className="space-y-4">
          <div>
            <label className="mb-1 block text-sm font-medium">New password</label>
            <Input
              type="password"
              value={passwordModal?.newPassword ?? ''}
              onChange={(e) =>
                passwordModal &&
                setPasswordModal({ ...passwordModal, newPassword: e.target.value })
              }
              placeholder="••••••••"
            />
          </div>
          {updatePassword.isError && (
            <p className="text-sm text-red-400">
              {(updatePassword.error as { response?: { data?: { message?: string } } })?.response
                ?.data?.message ?? 'Failed to change password'}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setPasswordModal(null)}>
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={!passwordModal?.newPassword.trim() || updatePassword.isPending}
            >
              {updatePassword.isPending ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
};
