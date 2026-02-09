import { Outlet } from 'react-router-dom';
import { Sidebar } from '@/components/layout/Sidebar';
import { Topbar } from '@/components/layout/Topbar';

export const MainLayout = () => {
  return (
    <div className="flex h-screen bg-bg-primary text-gray-100">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <Topbar />
        <main className="flex-1 overflow-y-auto scrollbar-thin">
          <div className="mx-auto max-w-[1600px] px-6 py-6 lg:px-8">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
};
