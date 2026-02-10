import { Link, useLocation } from 'react-router-dom';
import { LayoutDashboard, Database, Sparkles, Key, Users as UsersIcon, ShieldCheck, Cpu, Radio, BarChart3, Settings } from 'lucide-react';
import { cn } from '@/utils/cn';
import { getStoredRole, type Role } from '@/api/ferresdb';

const allNav = [
  { name: 'Overview', href: '/', icon: LayoutDashboard, roles: ['admin', 'editor', 'viewer'] as Role[] },
  { name: 'Collections', href: '/collections', icon: Database, roles: ['admin', 'editor', 'viewer'] as Role[] },
  { name: 'Analytics', href: '/analytics', icon: BarChart3, roles: ['admin', 'editor', 'viewer'] as Role[] },
  { name: 'Embeddings', href: '/embeddings', icon: Cpu, roles: ['admin', 'editor'] as Role[] },
  { name: 'Streaming', href: '/streaming', icon: Radio, roles: ['admin', 'editor'] as Role[] },
  { name: 'Query Tester', href: '/query-tester', icon: Sparkles, roles: ['admin', 'editor'] as Role[] },
  { name: 'API Keys', href: '/api-keys', icon: Key, roles: ['admin', 'editor'] as Role[] },
  { name: 'Users', href: '/users', icon: UsersIcon, roles: ['admin'] as Role[] },
  { name: 'Audit', href: '/audit', icon: ShieldCheck, roles: ['admin'] as Role[] },
  { name: 'Settings', href: '/settings', icon: Settings, roles: ['admin'] as Role[] },
];

export const Sidebar = () => {
  const location = useLocation();
  const role = getStoredRole();
  const navigation = role ? allNav.filter((item) => item.roles.includes(role)) : allNav;

  return (
    <aside className="flex h-full w-[240px] flex-col border-r border-white/[0.06] bg-bg-secondary">
      <div className="flex h-14 shrink-0 items-center gap-3 px-4 border-b border-white/[0.06]">
        <img src="/logo.png" alt="FerresDB" className="h-7 w-auto object-contain" />
      </div>
      <nav className="flex-1 overflow-y-auto scrollbar-thin px-3 py-4">
        <ul className="space-y-0.5">
          {navigation.map((item) => {
            const isActive = location.pathname === item.href;
            return (
              <li key={item.name}>
                <Link
                  to={item.href}
                  className={cn(
                    'flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors duration-150',
                    isActive
                      ? 'bg-white/[0.08] text-white'
                      : 'text-gray-400 hover:bg-white/[0.04] hover:text-gray-200'
                  )}
                >
                  <item.icon className="h-[18px] w-[18px] shrink-0 opacity-90" />
                  <span>{item.name}</span>
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>
    </aside>
  );
};
