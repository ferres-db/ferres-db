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
    <div className="flex h-16 items-center justify-between border-b border-bg-tertiary bg-bg-secondary px-6">
      <div className="flex items-center gap-4">
        <h2 className="text-lg font-semibold text-gray-50">Dashboard</h2>
      </div>
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm">
          <Bell className="h-5 w-5" />
        </Button>
        <Button variant="ghost" size="sm">
          <Settings className="h-5 w-5" />
        </Button>
        <Button variant="ghost" size="sm" onClick={handleLogout} title="Sair">
          <LogOut className="h-5 w-5" />
        </Button>
      </div>
    </div>
  );
};
