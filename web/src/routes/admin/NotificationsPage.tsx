import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useNotifications } from "@/api/hooks";

export function NotificationsPage() {
  const { data, isLoading, error } = useNotifications(true);
  if (isLoading) return <p>Loading…</p>;
  if (error) return <p className="text-sm text-destructive">Failed to load notifications.</p>;
  const rows = data ?? [];
  return (
    <div>
      <h1 className="mb-4 text-xl font-semibold">Notifications</h1>
      {rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">No notifications yet.</p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Sent</TableHead><TableHead>Template</TableHead><TableHead>Subject</TableHead>
              <TableHead>Channel</TableHead><TableHead>Recipient</TableHead>
              <TableHead>Body</TableHead><TableHead>Correlation</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((n) => (
              <TableRow key={n.id}>
                <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                  {new Date(n.created_at).toLocaleString()}
                </TableCell>
                <TableCell>{n.template}</TableCell>
                <TableCell>{n.subject}</TableCell>
                <TableCell>{n.channel}</TableCell>
                <TableCell>{n.recipient}</TableCell>
                <TableCell className="max-w-xs">
                  <details>
                    <summary className="cursor-pointer truncate text-xs text-muted-foreground">
                      {n.body.slice(0, 60)}
                    </summary>
                    <pre className="mt-1 whitespace-pre-wrap text-xs">{n.body}</pre>
                  </details>
                </TableCell>
                <TableCell className="text-xs text-muted-foreground">{n.created_by_cid}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
