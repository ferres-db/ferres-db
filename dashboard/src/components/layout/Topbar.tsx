import { useNavigate } from 'react-router-dom';
import { Bell, LogOut, Settings } from 'lucide-react';
import { Button } from '@/components/ui/Button';
import { clearStoredToken } from '@/api/ferresdb';

export const Topbar = () => {
  const navigate = useNavigate();

  const handleLogout = () => {
    clearStoredToken();
    navigate('/login', { replace: true });
  };

  return (
    <header className="flex h-14 shrink-0 items-center justify-between gap-4 border-b border-white/[0.06] bg-bg-secondary/80 px-6 backdrop-blur-sm">
      <div className="flex min-w-0 items-center gap-4">
        <h2 className="truncate text-sm font-semibold tracking-tight text-gray-50">Dashboard</h2>
      </div>
      <div className="flex items-center gap-1">
        <Button variant="ghost" size="sm" className="h-8 w-8 rounded-md p-0" title="Notificações">
          <Bell className="h-4 w-4" />
        </Button>
        <Button variant="ghost" size="sm" className="h-8 w-8 rounded-md p-0" title="Configurações">
          <Settings className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="h-8 w-8 rounded-md p-0"
          onClick={handleLogout}
          title="Sair"
        >
          <LogOut className="h-4 w-4" />
        </Button>
      </div>
    </header>
  );
};
