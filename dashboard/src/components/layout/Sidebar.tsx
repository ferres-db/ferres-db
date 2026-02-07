import { Link, useLocation } from 'react-router-dom';
import { LayoutDashboard, Database, Sparkles, Key, Users as UsersIcon, ShieldCheck, Cpu, Radio } from 'lucide-react';
import { cn } from '@/utils/cn';
import { getStoredRole, type Role } from '@/api/ferresdb';

const allNav = [
  { name: 'Overview', href: '/', icon: LayoutDashboard, roles: ['admin', 'editor', 'viewer'] as Role[] },
  { name: 'Collections', href: '/collections', icon: Database, roles: ['admin', 'editor', 'viewer'] as Role[] },
  { name: 'Embeddings', href: '/embeddings', icon: Cpu, roles: ['admin', 'editor'] as Role[] },
  { name: 'Streaming', href: '/streaming', icon: Radio, roles: ['admin', 'editor'] as Role[] },
  { name: 'Query Tester', href: '/query-tester', icon: Sparkles, roles: ['admin', 'editor'] as Role[] },
  { name: 'API Keys', href: '/api-keys', icon: Key, roles: ['admin', 'editor'] as Role[] },
  { name: 'Users', href: '/users', icon: UsersIcon, roles: ['admin'] as Role[] },
  { name: 'Audit', href: '/audit', icon: ShieldCheck, roles: ['admin'] as Role[] },
];

export const Sidebar = () => {
  const location = useLocation();
  const role = getStoredRole();
  const navigation = role ? allNav.filter((item) => item.roles.includes(role)) : allNav;

  return (
    <div className="flex h-full w-64 flex-col bg-bg-secondary text-gray-50">
      <div className="flex h-16 items-center px-6 border-b border-bg-tertiary">
        <h1 className="text-xl font-bold">FerresDB</h1>
      </div>
      <nav className="flex-1 space-y-1 px-3 py-4">
        {navigation.map((item) => {
          const isActive = location.pathname === item.href;
          return (
            <Link
              key={item.name}
              to={item.href}
              className={cn(
                'flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-bg-tertiary text-white'
                  : 'text-gray-400 hover:bg-bg-tertiary hover:text-white'
              )}
            >
              <item.icon className="h-5 w-5" />
              {item.name}
            </Link>
          );
        })}
      </nav>
    </div>
  );
};
