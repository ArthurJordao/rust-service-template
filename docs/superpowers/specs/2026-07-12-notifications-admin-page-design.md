# Notifications Admin Page — Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation planning
**Scope:** A `/admin/notifications` page in the SPA over the **existing** admin
`GET /notifications` endpoint, plus persisting the notification `subject`. This is
the "DB-only notifier" story: the app records every dispatched notification and an
admin can read it in the UI — no real email provider yet.

---

## 1. Goal & context

The notification backend is already built and event-driven:

- The `account.created` consumer renders a welcome body, calls the `Notifier` port
  (dev `LogNotifier` — logs only), and records a `sent_notification` row
  (`source_event_id`, `template`, `channel`, `recipient`, `body`, `created_at`,
  `created_by_cid`) idempotently.
- `GET /notifications` (admin scope) returns all rows as `Vec<SentNotification>`,
  already in the generated OpenAPI schema.

The **only** gap is UI: the SPA has `/admin/users` and `/admin/dlq` but no
notifications view. This spec closes that, and adds the one missing datum
(`subject`) so the admin sees what was actually sent.

**Principle:** reuse the existing admin-page kit (mirror `DlqPage` / `UsersPage`) —
TanStack Query hook + generated `schema.d.ts` types + `apiFetch`. No new libraries.

---

## 2. Decisions

1. **Persist `subject`.** Today the subject is the hardcoded string `"Welcome"`,
   passed to `Notifier::send` but never stored. Add a `subject` column so the
   record is self-describing and future templates differ visibly. Migration
   `0008_notification_subject.sql`.
2. **Read-only page.** No actions (no resend/delete) — notifications are an audit
   trail. Matches the DB-only-delivery intent.
3. **Body display:** truncated in the row with an expand/disclosure (bodies are
   short today but templates may grow); the correlation id is shown for tracing.

---

## 3. Backend changes

- **Migration `0008_notification_subject.sql`:** `alter table sent_notification add
  column subject text not null default ''`. (Default keeps existing rows valid; new
  rows always set it.)
- **Model/DTO:** add `subject: String` to `SentNotification` (and
  `NewSentNotification`).
- **Repository:** include `subject` in the `record` insert and the `list` select.
- **Consumer (`events.rs`):** pass the rendered subject into `NewSentNotification`
  (extract the current `"Welcome"` literal into the record; it already passes it to
  `notifier.send`).
- **OpenAPI:** `SentNotification` already exported; regenerate `web/src/api/schema.d.ts`
  via `make gen-api` so `subject` appears (the openapi-drift CI job must stay green).

No new endpoint — `GET /notifications` already returns the full set.

## 4. Frontend changes

- **`web/src/api/hooks.ts`:** `useNotifications(enabled)` — `useQuery` on
  `GET /notifications`, gated on `!!user` (same pattern as `useMfaStatus`), queryKey
  `["notifications"]`.
- **`web/src/routes/admin/NotificationsPage.tsx`:** a table mirroring `DlqPage` —
  columns: created_at, template, subject, channel, recipient, body (truncated +
  expand), correlation id. Empty state when none. Loading + error (toast with cid)
  states as in the other admin pages.
- **Routing (`App.tsx`):** add `<Route path="/admin/notifications" element={
  <NotificationsPage />} />` inside the existing `RequireAdmin` wrapper.
- **Nav:** add the "Notifications" link wherever `Users`/`DLQ` admin links live
  (the admin section of the layout/nav).

## 5. Testing strategy

Vitest + Testing Library + msw, following `UsersPage.test.tsx` / `DlqPage` tests:

- Renders the rows returned by a mocked `GET /notifications` (asserts subject,
  recipient, template appear).
- Empty state when the endpoint returns `[]`.
- Body expand toggle reveals the full body.
- Backend: extend the existing notification consumer test to assert the recorded
  row now carries the expected `subject`; a repo round-trip test that `subject`
  persists and comes back from `list`.

## 6. Files touched

- **Backend:** `migrations/0008_notification_subject.sql` (create);
  `crates/domain-notification/src/models.rs`,
  `crates/domain-notification/src/ports/{repository.rs,postgres.rs,events.rs}`;
  regenerate `web/src/api/schema.d.ts`.
- **Frontend:** `web/src/routes/admin/NotificationsPage.tsx` (create) + test;
  `web/src/api/hooks.ts`, `web/src/App.tsx`, the admin nav component.
