import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { useDeadLetters, useReplayDeadLetter } from "@/api/hooks";

export function DlqPage() {
  const { data, isLoading, error } = useDeadLetters();
  const replay = useReplayDeadLetter();
  if (isLoading) return <p>Loading…</p>;
  if (error) return <p className="text-sm text-destructive">Failed to load dead letters.</p>;
  const rows = data ?? [];
  return (
    <div>
      <h1 className="mb-4 text-xl font-semibold">Dead-letter queue</h1>
      {rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">No dead letters. 🎉</p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Subscriber</TableHead><TableHead>Event</TableHead><TableHead>Aggregate</TableHead>
              <TableHead>Attempts</TableHead><TableHead>Last error</TableHead><TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((d) => (
              <TableRow key={d.delivery_id}>
                <TableCell>{d.subscriber_name}</TableCell>
                <TableCell>{d.event_type}</TableCell>
                <TableCell>{d.aggregate_id}</TableCell>
                <TableCell>{d.attempts}</TableCell>
                <TableCell className="max-w-xs truncate text-xs text-muted-foreground">{d.last_error}</TableCell>
                <TableCell className="text-right">
                  <Button size="sm" disabled={replay.isPending} onClick={() => replay.mutate(d.delivery_id)}>Replay</Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
