import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useUsers, useCreateUser } from '@/hooks/useUsers';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Modal } from '@/components/ui/Modal';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Skeleton } from '@/components/ui/Skeleton';
import { Plus, Trash2, Key } from 'lucide-react';
import { format } from 'date-fns';
import type { UserInfo } from '@/types';
import { useDeleteUser, useUpdateUserPassword } from '@/hooks/useUsers';

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
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [newUsername, setNewUsername] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [newUserRole, setNewUserRole] = useState<string>('viewer');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [passwordModal, setPasswordModal] = useState<{ user: UserInfo; newPassword: string } | null>(null);

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
      });
      setIsCreateModalOpen(false);
      setNewUsername('');
      setNewPassword('');
      setNewUserRole('viewer');
      setConfirmPassword('');
    } catch {
      // Error shown in modal
    }
  };

  const handleDelete = async (u: UserInfo) => {
    if (!confirm(`Remover usuário "${u.username}"?`)) return;
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

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">Usuários</h1>
          <p className="text-gray-600 mt-2">Usuários que podem acessar o dashboard</p>
        </div>
        <Button onClick={() => setIsCreateModalOpen(true)}>
          <Plus className="h-4 w-4 mr-2" />
          Novo usuário
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Usuários do dashboard</CardTitle>
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
                  <TableHead>Usuário</TableHead>
                  <TableHead>Role</TableHead>
                  <TableHead>Criado em</TableHead>
                  <TableHead>Ações</TableHead>
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
                      <TableCell>{format(new Date(user.created_at * 1000), 'dd/MM/yyyy HH:mm')}</TableCell>
                      <TableCell>
                        <div className="flex gap-2">
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() => setPasswordModal({ user, newPassword: '' })}
                            title="Alterar senha"
                          >
                            <Key className="h-4 w-4" />
                          </Button>
                          <Button
                            variant="danger"
                            size="sm"
                            onClick={() => handleDelete(user)}
                            disabled={deleteUser.isPending}
                            title="Remover usuário"
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                {(!users || !Array.isArray(users) || users.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={4} className="text-center text-gray-500 py-8">
                      Nenhum usuário além do padrão. Crie um novo usuário.
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
        title="Novo usuário"
      >
        <form onSubmit={handleCreate} className="space-y-4">
          <div>
            <label className="mb-1 block text-sm font-medium">Usuário</label>
            <Input
              value={newUsername}
              onChange={(e) => setNewUsername(e.target.value)}
              placeholder="nome_usuario"
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
            <label className="mb-1 block text-sm font-medium">Senha</label>
            <Input
              type="password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              placeholder="••••••••"
              autoComplete="new-password"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium">Confirmar senha</label>
            <Input
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              placeholder="••••••••"
              autoComplete="new-password"
            />
          </div>
          {newPassword && confirmPassword && newPassword !== confirmPassword && (
            <p className="text-sm text-red-400">As senhas não coincidem.</p>
          )}
          {createUser.isError && (
            <p className="text-sm text-red-400">
              {(createUser.error as { response?: { data?: { message?: string } } })?.response?.data?.message ||
                (createUser.error instanceof Error ? createUser.error.message : 'Erro ao criar usuário')}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setIsCreateModalOpen(false)}>
              Cancelar
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
              {createUser.isPending ? 'Criando…' : 'Criar'}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        isOpen={!!passwordModal}
        onClose={() => setPasswordModal(null)}
        title={`Alterar senha: ${passwordModal?.user.username ?? ''}`}
      >
        <form onSubmit={handleChangePassword} className="space-y-4">
          <div>
            <label className="mb-1 block text-sm font-medium">Nova senha</label>
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
                ?.data?.message ?? 'Erro ao alterar senha'}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={() => setPasswordModal(null)}>
              Cancelar
            </Button>
            <Button
              type="submit"
              disabled={!passwordModal?.newPassword.trim() || updatePassword.isPending}
            >
              {updatePassword.isPending ? 'Salvando…' : 'Salvar'}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
};
