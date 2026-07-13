# Notifications Admin Page — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/admin/notifications` SPA page over the existing admin `GET /notifications` endpoint, and persist the notification `subject`.

**Architecture:** Backend already records `sent_notification` rows and serves them at `GET /notifications` (admin scope). Task 1 adds the `subject` column end-to-end (migration → model → repo → consumer). Task 2 adds the SPA page (regen typed client → api wrapper → hook → page → route → nav), mirroring the existing `DlqPage`.

**Tech Stack:** Rust (axum 0.7, sqlx 0.8 runtime API), React 19 + TanStack Query + generated OpenAPI types, vitest + msw.

## Global Constraints

- **sqlx runtime query API** (`sqlx::query`, `query_as`, `.bind`) — never the `query!` macros.
- Run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` before every Rust commit; both must be clean.
- Web checks: `CI=true npm --prefix web run lint`, `CI=true npm --prefix web run build`, `CI=true npm --prefix web test`.
- `make gen-api` regenerates `web/src/api/schema.d.ts`; the openapi-drift CI job must stay green (commit the regenerated file).
- `#[sqlx::test]` integration tests use `#[sqlx::test(migrations = "../../migrations")]`.
- One commit per task.

---

### Task 1: Persist the notification `subject` (backend)

**Files:**
- Create: `migrations/0008_notification_subject.sql`
- Modify: `crates/domain-notification/src/models.rs`
- Modify: `crates/domain-notification/src/ports/postgres.rs`
- Modify: `crates/domain-notification/src/ports/events.rs`
- Test: `crates/domain-notification/tests/repository.rs`, `crates/domain-notification/tests/subscriber.rs`

**Interfaces:**
- Consumes: existing `SentNotification` / `NewSentNotification` structs, `SentNotificationRepository`, `NotificationSubscriber`.
- Produces: `SentNotification.subject: String` and `NewSentNotification.subject: String`; the `sent_notification` table gains a `subject text not null default ''` column; the welcome consumer stores `subject = "Welcome"`.

- [ ] **Step 1: Write the migration**

Create `migrations/0008_notification_subject.sql`:

```sql
alter table sent_notification add column subject text not null default '';
```

- [ ] **Step 2: Update the failing repository test**

In `crates/domain-notification/tests/repository.rs`, add `subject` to `new_row` and assert it round-trips:

```rust
fn new_row(event_id: i64) -> NewSentNotification {
    NewSentNotification {
        source_event_id: event_id,
        template: "welcome".into(),
        subject: "Welcome".into(),
        channel: "email".into(),
        recipient: "a@b.c".into(),
        body: "hi".into(),
        created_by_cid: "cid".into(),
    }
}
```

In `record_find_list`, after fetching `found`, add:

```rust
    assert_eq!(found.subject, "Welcome");
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `DATABASE_URL=… cargo test -p domain-notification --test repository`
Expected: FAIL to compile — `NewSentNotification` has no field `subject`.

- [ ] **Step 4: Add `subject` to the models**

In `crates/domain-notification/src/models.rs`, add `pub subject: String,` after `template` in **both** `SentNotification` and `NewSentNotification`:

```rust
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SentNotification {
    pub id: i64,
    pub source_event_id: i64,
    pub template: String,
    pub subject: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewSentNotification {
    pub source_event_id: i64,
    pub template: String,
    pub subject: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_by_cid: String,
}
```

- [ ] **Step 5: Update the repository SQL**

In `crates/domain-notification/src/ports/postgres.rs`, add `subject` to `COLS` and to the `record` insert:

```rust
const COLS: &str =
    "id, source_event_id, template, subject, channel, recipient, body, created_at, created_by_cid";
```

```rust
    async fn record(&self, new: NewSentNotification) -> anyhow::Result<()> {
        sqlx::query(
            "insert into sent_notification \
             (source_event_id, template, subject, channel, recipient, body, created_by_cid) \
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(new.source_event_id)
        .bind(&new.template)
        .bind(&new.subject)
        .bind(&new.channel)
        .bind(&new.recipient)
        .bind(&new.body)
        .bind(&new.created_by_cid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
```

- [ ] **Step 6: Set `subject` in the consumer**

In `crates/domain-notification/src/ports/events.rs`, bind the subject once and use it for both the notifier and the record. Replace the `notifier.send(...)` + `record(...)` block:

```rust
        let subject = "Welcome";
        let channel = NotificationChannel::Email(payload.email.clone());
        self.notifier.send(&channel, subject, &body).await?;

        let (kind, recipient) = channel.parts();
        self.repo
            .record(NewSentNotification {
                source_event_id: event.event_id,
                template: "welcome".into(),
                subject: subject.into(),
                channel: kind.into(),
                recipient: recipient.into(),
                body,
                created_by_cid: event.correlation_id.clone(),
            })
            .await?;
```

- [ ] **Step 7: Assert `subject` in the subscriber test**

In `crates/domain-notification/tests/subscriber.rs`, in `sends_and_records_then_is_idempotent`, after the existing `assert_eq!(row.template, "welcome");`:

```rust
    assert_eq!(row.subject, "Welcome");
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `DATABASE_URL=… cargo test -p domain-notification`
Expected: PASS (repository + subscriber).

- [ ] **Step 9: fmt, clippy, commit**

Run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` (both clean), then:

```bash
git add migrations/0008_notification_subject.sql crates/domain-notification/
git commit -m "feat(notification): persist notification subject"
```

---

### Task 2: Notifications admin page (frontend)

**Files:**
- Modify: `web/src/api/schema.d.ts` (regenerated — do not hand-edit)
- Modify: `web/src/api/types.ts`
- Create: `web/src/api/notifications.ts`
- Modify: `web/src/api/hooks.ts`
- Create: `web/src/routes/admin/NotificationsPage.tsx`
- Create: `web/src/routes/admin/NotificationsPage.test.tsx`
- Modify: `web/src/App.tsx`
- Modify: `web/src/components/AppLayout.tsx`

**Interfaces:**
- Consumes: `SentNotification` from the regenerated schema (now includes `subject`); `apiFetch`; the existing `Table` UI components and `useMe`-style `enabled` gating.
- Produces: `useNotifications(enabled)` hook (queryKey `["notifications"]`), `/admin/notifications` route, a "Notifications" admin nav link.

- [ ] **Step 1: Regenerate the typed client**

Run: `make gen-api`
Expected: `web/src/api/schema.d.ts` updates so `components["schemas"]["SentNotification"]` includes `subject: string`. (Requires Task 1 merged so the OpenAPI reflects `subject`.)

- [ ] **Step 2: Add the type + api wrapper**

In `web/src/api/types.ts` add:

```ts
export type SentNotification = components["schemas"]["SentNotification"];
```

Create `web/src/api/notifications.ts`:

```ts
import { apiFetch } from "@/lib/fetchClient";
import type { SentNotification } from "@/api/types";

export const listNotifications = () => apiFetch<SentNotification[]>("/notifications");
```

- [ ] **Step 3: Add the hook**

In `web/src/api/hooks.ts`, add the import and the hook (mirror `useMfaStatus`'s `enabled` gating):

```ts
import { listNotifications } from "@/api/notifications";
```

```ts
export function useNotifications(enabled: boolean) {
  return useQuery({ queryKey: ["notifications"], queryFn: listNotifications, enabled });
}
```

- [ ] **Step 4: Write the failing page test**

Create `web/src/routes/admin/NotificationsPage.test.tsx` (follow `UsersPage.test.tsx` for the msw + render harness; the app's real auth gating means the page should be rendered with a logged-in user — reuse whatever helper `UsersPage.test.tsx` uses to render an admin page). Assert the mocked rows render and the empty state shows:

```tsx
import { http, HttpResponse } from "msw";
import { screen } from "@testing-library/react";
// import the shared render + server helpers exactly as UsersPage.test.tsx does

test("renders notification rows", async () => {
  server.use(
    http.get("/api/notifications", () =>
      HttpResponse.json([
        {
          id: 1, source_event_id: 5, template: "welcome", subject: "Welcome",
          channel: "email", recipient: "a@b.c", body: "hello there",
          created_at: "2026-07-12T10:00:00Z", created_by_cid: "root.ab",
        },
      ]),
    ),
  );
  renderAdmin(<NotificationsPage />); // use the project's admin-render helper
  expect(await screen.findByText("a@b.c")).toBeInTheDocument();
  expect(screen.getByText("Welcome")).toBeInTheDocument();
});

test("shows empty state", async () => {
  server.use(http.get("/api/notifications", () => HttpResponse.json([])));
  renderAdmin(<NotificationsPage />);
  expect(await screen.findByText(/no notifications/i)).toBeInTheDocument();
});
```

Note for the implementer: match `UsersPage.test.tsx`'s exact import paths for `server` and the render helper; do not invent a new harness.

- [ ] **Step 5: Run the test to verify it fails**

Run: `CI=true npm --prefix web test -- NotificationsPage`
Expected: FAIL — `NotificationsPage` does not exist.

- [ ] **Step 6: Build the page**

Create `web/src/routes/admin/NotificationsPage.tsx` (mirror `DlqPage.tsx`; gate the query on an authenticated user the same way other admin pages do — pass `true` if the admin route wrapper already guarantees a user, matching `useDeadLetters()` which passes no gate):

```tsx
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
```

- [ ] **Step 7: Wire the route**

In `web/src/App.tsx`, add inside the existing `RequireAdmin` block (next to the DLQ route):

```tsx
import { NotificationsPage } from "@/routes/admin/NotificationsPage";
```

```tsx
              <Route path="/admin/notifications" element={<NotificationsPage />} />
```

- [ ] **Step 8: Add the nav link**

In `web/src/components/AppLayout.tsx`, next to the existing DLQ link:

```tsx
          {isAdmin && <Link to="/admin/notifications">Notifications</Link>}
```

- [ ] **Step 9: Run the page test + full web checks**

Run: `CI=true npm --prefix web test -- NotificationsPage` → PASS.
Then the full gate:
`CI=true npm --prefix web run lint` (clean), `CI=true npm --prefix web run build` (clean, no `tsc` errors), `CI=true npm --prefix web test` (all pass).

- [ ] **Step 10: Commit**

```bash
git add web/
git commit -m "feat(web): admin notifications page"
```
