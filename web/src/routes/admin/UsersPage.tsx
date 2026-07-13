import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { useUsers } from "@/api/hooks";
import { EditScopesDialog } from "@/routes/admin/EditScopesDialog";
import { ResetMfaDialog } from "@/routes/admin/ResetMfaDialog";

export function UsersPage() {
  const { data, isLoading, error } = useUsers();
  if (isLoading) return <p>Loading…</p>;
  if (error) return <p className="text-sm text-destructive">Failed to load users.</p>;
  return (
    <div>
      <h1 className="mb-4 text-xl font-semibold">Users</h1>
      <Table>
        <TableHeader>
          <TableRow><TableHead>ID</TableHead><TableHead>Email</TableHead><TableHead>Scopes</TableHead><TableHead /></TableRow>
        </TableHeader>
        <TableBody>
          {(data ?? []).map((u) => (
            <TableRow key={u.id}>
              <TableCell>{u.id}</TableCell>
              <TableCell>{u.email}</TableCell>
              <TableCell className="space-x-1">{u.scopes.map((s) => <Badge key={s} variant="secondary">{s}</Badge>)}</TableCell>
              <TableCell className="text-right space-x-2">
                <EditScopesDialog user={u} />
                <ResetMfaDialog userId={u.id} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
