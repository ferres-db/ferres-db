import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { authApi, setStoredToken, setStoredRole } from '@/api/ferresdb';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';

export const Login = () => {
  const navigate = useNavigate();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');
    if (!username.trim() || !password) {
      setError('Preencha usuário e senha');
      return;
    }
    setLoading(true);
    try {
      const { token, role } = await authApi.login(username.trim(), password);
      setStoredToken(token);
      setStoredRole(role ?? 'viewer');
      navigate('/', { replace: true });
    } catch (err: unknown) {
      const msg =
        (err as { response?: { data?: { message?: string } } })?.response?.data?.message ||
        'Usuário ou senha inválidos';
      setError(msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-bg-primary p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-center text-xl">FerresDB Dashboard</CardTitle>
          <p className="text-center text-sm text-gray-500">Entre com sua conta</p>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
            <div>
              <label className="mb-1 block text-sm font-medium">Usuário</label>
              <Input
                type="text"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                placeholder="root"
                autoComplete="username"
                autoFocus
              />
            </div>
            <div>
              <label className="mb-1 block text-sm font-medium">Senha</label>
              <Input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="••••••••"
                autoComplete="current-password"
              />
            </div>
            {error && <p className="text-sm text-red-400">{error}</p>}
            <Button type="submit" className="w-full" disabled={loading}>
              {loading ? 'Entrando…' : 'Entrar'}
            </Button>
          </form>
          <p className="mt-4 text-center text-xs text-gray-500">
            Padrão: usuário <code className="rounded bg-bg-tertiary px-1">root</code>, senha{' '}
            <code className="rounded bg-bg-tertiary px-1">ferresdb</code>
          </p>
        </CardContent>
      </Card>
    </div>
  );
};
