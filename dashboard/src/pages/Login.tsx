import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { authApi, setStoredToken, setStoredRole } from '@/api/ferresdb';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Card, CardContent, CardHeader } from '@/components/ui/Card';

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
      setError('Enter username and password');
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
        'Invalid username or password';
      setError(msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-bg-primary p-4">
      <Card className="w-full max-w-[400px] border-white/[0.08] shadow-modal">
        <CardHeader className="text-center">
          <div className="flex justify-center">
            <img src="/logo.png" alt="FerresDB" className="h-9 w-auto object-contain" />
          </div>
          <p className="mt-3 text-sm text-gray-400">Sign in to your account</p>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
            <div>
              <label className="mb-1.5 block text-sm font-medium text-gray-300">Username</label>
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
              <label className="mb-1.5 block text-sm font-medium text-gray-300">Password</label>
              <Input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="••••••••"
                autoComplete="current-password"
              />
            </div>
            {error && (
              <p className="rounded-lg bg-error/10 px-3 py-2 text-sm text-red-400">{error}</p>
            )}
            <Button type="submit" className="w-full" disabled={loading}>
              {loading ? 'Signing in…' : 'Sign in'}
            </Button>
          </form>
          <p className="mt-5 text-center text-xs text-gray-500">
            Default: username <code className="rounded bg-white/[0.08] px-1.5 py-0.5 font-mono text-gray-400">root</code>, password{' '}
            <code className="rounded bg-white/[0.08] px-1.5 py-0.5 font-mono text-gray-400">ferresdb</code>
          </p>
        </CardContent>
      </Card>
    </div>
  );
};
