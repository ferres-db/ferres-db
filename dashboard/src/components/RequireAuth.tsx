import { Navigate, Outlet, useLocation } from 'react-router-dom';
import { clearStoredToken, getStoredToken } from '@/api/ferresdb';

/** Verifica se um JWT está expirado decodificando o payload sem verificar assinatura. */
function isTokenExpired(token: string): boolean {
  try {
    const parts = token.split('.');
    if (parts.length !== 3) return true;
    const payload = JSON.parse(atob(parts[1]));
    if (!payload.exp) return false;
    return Date.now() / 1000 > payload.exp;
  } catch {
    return true;
  }
}

/** Redireciona para /login se não houver token ou se o token estiver expirado. */
export const RequireAuth = () => {
  const token = getStoredToken();
  const location = useLocation();

  if (!token || isTokenExpired(token)) {
    if (token) clearStoredToken();
    return <Navigate to="/login" state={{ from: location }} replace />;
  }

  return <Outlet />;
};
