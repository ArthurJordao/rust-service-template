import { useAuth } from "@/auth/AuthProvider";
import { useMe } from "@/api/hooks";
import { Card } from "@/components/ui/card";

export function AccountPage() {
  const { user } = useAuth();
  const { data, isLoading, error } = useMe(!!user);
  return (
    <Card className="max-w-md p-6">
      <h1 className="mb-4 text-xl font-semibold">My account</h1>
      {isLoading && <p>Loading…</p>}
      {error && (
        <p className="text-sm text-muted-foreground">
          No account yet for {user?.email}.
        </p>
      )}
      {data && (
        <dl className="space-y-2 text-sm">
          <div>
            <dt className="text-muted-foreground">Email</dt>
            <dd>{data.email}</dd>
          </div>
          <div>
            <dt className="text-muted-foreground">Account ID</dt>
            <dd>{data.id}</dd>
          </div>
          <div>
            <dt className="text-muted-foreground">Created</dt>
            <dd>{new Date(data.created_at).toLocaleString()}</dd>
          </div>
        </dl>
      )}
    </Card>
  );
}
