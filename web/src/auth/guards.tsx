import { Navigate, Outlet } from "react-router-dom";
import { useAuth } from "@/auth/AuthProvider";

export function RequireAuth() {
  const { user, status } = useAuth();
  if (status === "loading") return <p className="p-8">Loading…</p>;
  return user ? <Outlet /> : <Navigate to="/login" replace />;
}

export function RequireAdmin() {
  const { user, status } = useAuth();
  if (status === "loading") return <p className="p-8">Loading…</p>;
  if (!user) return <Navigate to="/login" replace />;
  return user.scopes.includes("admin") ? <Outlet /> : <Navigate to="/" replace />;
}
