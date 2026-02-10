import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MainLayout } from '@/components/layout/MainLayout';
import { RequireAuth } from '@/components/RequireAuth';
import { Overview } from '@/pages/Overview';
import { Collections } from '@/pages/Collections';
import { CollectionDetails } from '@/pages/CollectionDetails';
import { CollectionPoints } from '@/pages/CollectionPoints';
import { QueryTester } from '@/pages/QueryTester';
import { Embeddings } from '@/pages/Embeddings';
import { Streaming } from '@/pages/Streaming';
import { ApiKeys } from '@/pages/ApiKeys';
import { Users } from '@/pages/Users';
import { Audit } from '@/pages/Audit';
import { Analytics } from '@/pages/Analytics';
import { Settings } from '@/pages/Settings';
import { Login } from '@/pages/Login';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route path="/login" element={<Login />} />
          <Route element={<RequireAuth />}>
            <Route element={<MainLayout />}>
              <Route path="/" element={<Overview />} />
              <Route path="/collections" element={<Collections />} />
              <Route path="/collections/:name" element={<CollectionDetails />} />
              <Route path="/collections/:name/points" element={<CollectionPoints />} />
              <Route path="/embeddings" element={<Embeddings />} />
              <Route path="/streaming" element={<Streaming />} />
              <Route path="/query-tester" element={<QueryTester />} />
              <Route path="/api-keys" element={<ApiKeys />} />
              <Route path="/users" element={<Users />} />
              <Route path="/audit" element={<Audit />} />
              <Route path="/analytics" element={<Analytics />} />
              <Route path="/settings" element={<Settings />} />
            </Route>
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}

export default App;
