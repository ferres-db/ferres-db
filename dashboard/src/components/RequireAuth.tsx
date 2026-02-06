import { Navigate, Outlet, useLocation } from 'react-router-dom';
import { getStoredToken } from '@/api/ferresdb';

/** Redireciona para /login se não houver token. */
export const RequireAuth = () => {
  const token = getStoredToken();
  const location = useLocation();

  if (!token) {
    return <Navigate to="/login" state={{ from: location }} replace />;
  }

  return <Outlet />;
};
