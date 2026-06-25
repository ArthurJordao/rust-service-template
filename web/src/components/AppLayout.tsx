import { Link, Outlet, useNavigate } from "react-router-dom";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/button";

export function AppLayout() {
  const { user, isAdmin, logout } = useAuth();
  const navigate = useNavigate();
  return (
    <div className="min-h-screen">
      <nav className="flex items-center justify-between border-b px-6 py-3">
        <div className="flex gap-4">
          <Link to="/" className="font-semibold">
            Account
          </Link>
          {isAdmin && <Link to="/admin/users">Users</Link>}
          {isAdmin && <Link to="/admin/dlq">DLQ</Link>}
        </div>
        <div className="flex items-center gap-3">
          <span className="text-sm text-muted-foreground">{user?.email}</span>
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              await logout();
              navigate("/login");
            }}
          >
            Log out
          </Button>
        </div>
      </nav>
      <main className="p-6">
        <Outlet />
      </main>
    </div>
  );
}
