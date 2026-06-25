# Spec 3c: Admin Route Group (users + DLQ) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `/admin/*` route group to the SPA: a users page (list + edit-scopes dialog) and a DLQ page (list dead letters + replay), gated by `RequireAdmin`, with TanStack Query hooks and an MSW-backed test for an admin mutation.

**Architecture:** New `api/users.ts` + `api/dlq.ts` modules and query/mutation hooks; two pages built from shadcn table/dialog/badge/checkbox primitives; wired into the router under `<RequireAdmin>` inside the existing `AppLayout`.

**Tech Stack:** React 19 + TS, react-router v7, @tanstack/react-query, shadcn/ui, sonner; Vitest + RTL + MSW.

## Global Constraints

- Depends on Spec 3b (fetchClient, AuthProvider, AppLayout, routing, guards) and Spec 3a (`/api/users`, `/api/users/:id/scopes`, `/api/scopes`, `/api/admin/dlq`).
- All API calls via `apiFetch`. Admin pages live under `<RequireAdmin>`; the server also enforces `admin`.
- Mutations invalidate the relevant query and toast on success/error (sonner).
- TS `strict`. ESLint + Prettier clean. `npm run build` + `npm test -- run` must pass.
- Work in `web/`. Commit prefix `feat(web):` / `test(web):`.

---

### Task 1: users + dlq api modules, types, hooks

**Files:**
- Modify: `web/src/api/types.ts` (add admin types)
- Create: `web/src/api/users.ts`, `web/src/api/dlq.ts`
- Modify: `web/src/api/hooks.ts` (add admin hooks)

**Interfaces:**
- Produces:
  - types: `UserWithScopes { id: number; email: string; scopes: string[] }`, `ScopeInfo { id: number; name: string; description: string }`, `DeadLetter { delivery_id: number; subscriber_name: string; event_type: string; aggregate_id: string; payload: unknown; last_error: string | null; attempts: number }`
  - api: `listUsers()`, `listScopes()`, `setUserScopes(id, scopes)`, `listDeadLetters()`, `replayDeadLetter(deliveryId)`
  - hooks: `useUsers()`, `useScopes()`, `useSetUserScopes()`, `useDeadLetters()`, `useReplayDeadLetter()`

- [ ] **Step 1: Add admin types**

Append to `web/src/api/types.ts`:
```ts
export interface UserWithScopes {
  id: number;
  email: string;
  scopes: string[];
}
export interface ScopeInfo {
  id: number;
  name: string;
  description: string;
}
export interface DeadLetter {
  delivery_id: number;
  subscriber_name: string;
  event_type: string;
  aggregate_id: string;
  payload: unknown;
  last_error: string | null;
  attempts: number;
}
```

- [ ] **Step 2: api modules**

`web/src/api/users.ts`:
```ts
import { apiFetch } from "@/lib/fetchClient";
import type { ScopeInfo, UserWithScopes } from "@/api/types";

export const listUsers = () => apiFetch<UserWithScopes[]>("/users");
export const listScopes = () => apiFetch<ScopeInfo[]>("/scopes");
export const setUserScopes = (id: number, scopes: string[]) =>
  apiFetch<void>(`/users/${id}/scopes`, { method: "PUT", body: { scopes } });
```
`web/src/api/dlq.ts`:
```ts
import { apiFetch } from "@/lib/fetchClient";
import type { DeadLetter } from "@/api/types";

export const listDeadLetters = () => apiFetch<DeadLetter[]>("/admin/dlq");
export const replayDeadLetter = (deliveryId: number) =>
  apiFetch<{ replayed: boolean }>(`/admin/dlq/${deliveryId}/replay`, { method: "POST" });
```

- [ ] **Step 3: hooks**

Append to `web/src/api/hooks.ts`:
```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { listScopes, listUsers, setUserScopes } from "@/api/users";
import { listDeadLetters, replayDeadLetter } from "@/api/dlq";

export function useUsers() {
  return useQuery({ queryKey: ["users"], queryFn: listUsers });
}
export function useScopes() {
  return useQuery({ queryKey: ["scopes"], queryFn: listScopes });
}
export function useSetUserScopes() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, scopes }: { id: number; scopes: string[] }) => setUserScopes(id, scopes),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ["users"] }); toast.success("Scopes updated"); },
    onError: () => toast.error("Failed to update scopes"),
  });
}
export function useDeadLetters() {
  return useQuery({ queryKey: ["dlq"], queryFn: listDeadLetters });
}
export function useReplayDeadLetter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (deliveryId: number) => replayDeadLetter(deliveryId),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ["dlq"] }); toast.success("Replayed"); },
    onError: () => toast.error("Replay failed"),
  });
}
```
(Keep the existing `useMe` import block; merge imports so there's a single `@tanstack/react-query` import line — ESLint will flag duplicates.)

- [ ] **Step 4: Verify build**

In `web/`: `npm run build`
Expected: succeeds (no UI yet — modules compile).

- [ ] **Step 5: Commit**

```bash
git add web/src/api
git commit -m "feat(web): admin api modules (users, dlq) + query/mutation hooks"
```

---

### Task 2: UsersPage (table + edit-scopes dialog)

**Files:**
- Add shadcn UI: table, dialog, badge, checkbox
- Create: `web/src/routes/admin/UsersPage.tsx`, `web/src/routes/admin/EditScopesDialog.tsx`

**Interfaces:**
- Consumes: `useUsers`, `useScopes`, `useSetUserScopes`, shadcn primitives.
- Produces: `UsersPage` — table of users with scope badges + per-row "Edit scopes".

- [ ] **Step 1: Add shadcn primitives**

In `web/`:
```bash
npx shadcn@latest add table dialog badge checkbox
```
(Manual fallback from shadcn "new-york" source if non-interactive.)

- [ ] **Step 2: EditScopesDialog**

`web/src/routes/admin/EditScopesDialog.tsx`:
```tsx
import { useState } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { useScopes, useSetUserScopes } from "@/api/hooks";
import type { UserWithScopes } from "@/api/types";

export function EditScopesDialog({ user }: { user: UserWithScopes }) {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>(user.scopes);
  const { data: catalog } = useScopes();
  const setScopes = useSetUserScopes();

  function toggle(name: string, on: boolean) {
    setSelected((s) => (on ? [...new Set([...s, name])] : s.filter((x) => x !== name)));
  }

  return (
    <Dialog open={open} onOpenChange={(o) => { setOpen(o); if (o) setSelected(user.scopes); }}>
      <DialogTrigger asChild><Button variant="outline" size="sm">Edit scopes</Button></DialogTrigger>
      <DialogContent>
        <DialogHeader><DialogTitle>Scopes for {user.email}</DialogTitle></DialogHeader>
        <div className="space-y-2">
          {(catalog ?? []).map((s) => (
            <label key={s.id} className="flex items-center gap-2">
              <Checkbox checked={selected.includes(s.name)} onCheckedChange={(v) => toggle(s.name, !!v)} />
              <Label className="font-normal">{s.name}<span className="ml-2 text-xs text-muted-foreground">{s.description}</span></Label>
            </label>
          ))}
        </div>
        <DialogFooter>
          <Button
            disabled={setScopes.isPending}
            onClick={() => setScopes.mutate({ id: user.id, scopes: selected }, { onSuccess: () => setOpen(false) })}
          >Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```

- [ ] **Step 3: UsersPage**

`web/src/routes/admin/UsersPage.tsx`:
```tsx
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { useUsers } from "@/api/hooks";
import { EditScopesDialog } from "@/routes/admin/EditScopesDialog";

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
              <TableCell className="text-right"><EditScopesDialog user={u} /></TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
```

- [ ] **Step 4: Build + lint**

In `web/`: `npm run build && npm run lint` → pass.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): admin UsersPage + edit-scopes dialog"
```

---

### Task 3: DlqPage (table + replay)

**Files:**
- Create: `web/src/routes/admin/DlqPage.tsx`

**Interfaces:**
- Consumes: `useDeadLetters`, `useReplayDeadLetter`, shadcn table/button.
- Produces: `DlqPage` — table of dead letters + per-row Replay.

- [ ] **Step 1: DlqPage**

`web/src/routes/admin/DlqPage.tsx`:
```tsx
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
```

- [ ] **Step 2: Build + lint**

In `web/`: `npm run build && npm run lint` → pass.

- [ ] **Step 3: Commit**

```bash
git add web/src
git commit -m "feat(web): admin DlqPage with replay"
```

---

### Task 4: Wire admin routes + admin mutation test

**Files:**
- Modify: `web/src/App.tsx` (admin routes under `RequireAdmin`)
- Create: `web/src/routes/admin/UsersPage.test.tsx`

**Interfaces:**
- Consumes: `RequireAdmin`, `UsersPage`, `DlqPage`.
- Produces: reachable `/admin/users` + `/admin/dlq`; a test proving the set-scopes mutation flow against MSW.

- [ ] **Step 1: Add admin routes**

In `web/src/App.tsx`, import the guard + pages and add inside the `AppLayout` element, after the `/` route:
```tsx
import { RequireAdmin } from "@/auth/guards";
import { UsersPage } from "@/routes/admin/UsersPage";
import { DlqPage } from "@/routes/admin/DlqPage";
```
```tsx
          <Route element={<RequireAdmin />}>
            <Route path="/admin/users" element={<UsersPage />} />
            <Route path="/admin/dlq" element={<DlqPage />} />
          </Route>
```
(These nest inside the existing `<Route element={<AppLayout />}>` so they get the nav shell, and inside `<RequireAuth>`; `RequireAdmin` adds the scope gate.)

- [ ] **Step 2: Write the admin mutation test**

`web/src/routes/admin/UsersPage.test.tsx`:
```tsx
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { server } from "@/test/setup";
import { UsersPage } from "@/routes/admin/UsersPage";

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><UsersPage /></QueryClientProvider>);
}

describe("UsersPage", () => {
  it("lists users and saves edited scopes", async () => {
    let putBody: unknown = null;
    server.use(
      http.get("/api/users", () => HttpResponse.json([{ id: 1, email: "a@b.c", scopes: ["read:accounts:own"] }])),
      http.get("/api/scopes", () => HttpResponse.json([
        { id: 1, name: "admin", description: "all" },
        { id: 2, name: "read:accounts:own", description: "own" },
      ])),
      http.put("/api/users/1/scopes", async ({ request }) => { putBody = await request.json(); return new HttpResponse(null, { status: 204 }); }),
    );

    renderPage();
    await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());

    await userEvent.click(screen.getByText("Edit scopes"));
    await waitFor(() => screen.getByText("Scopes for a@b.c"));
    await userEvent.click(screen.getByLabelText(/admin/i));   // check admin
    await userEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(putBody).toEqual({ scopes: expect.arrayContaining(["read:accounts:own", "admin"]) }));
  });
});
```

- [ ] **Step 3: Run test + build**

In `web/`: `npm test -- run` then `npm run build && npm run lint`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add web/src
git commit -m "feat(web): wire /admin routes + admin mutation test"
```

---

### Task 5: Build the SPA and verify the app serves it

**Files:** none (verification)

**Interfaces:** confirms the 3a `ServeDir` fallback serves the built SPA.

- [ ] **Step 1: Build the SPA**

In `web/`: `npm run build`  → produces `web/dist/index.html` + assets.

- [ ] **Step 2: Verify the Rust app serves it (manual smoke)**

From repo root, with Postgres running and env configured:
```bash
DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres \
APP__DATABASE__AUTO_MIGRATE=true \
APP__SERVER__PORT=8080 APP__SERVER__ENVIRONMENT=local \
APP__AUTH__JWT_PUBLIC_KEY_PEM="$(cat crates/domain-auth/tests/fixtures/test_pub.pem)" \
APP__AUTH__JWT_PRIVATE_KEY_PEM="$(cat crates/domain-auth/tests/fixtures/test_priv.pem)" \
cargo run -p app &
sleep 3
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8080/          # 200 (index.html)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8080/admin/dlq # 200 (SPA fallback, not API)
curl -s http://localhost:8080/status                                     # OK
curl -s -X POST http://localhost:8080/api/auth/login -H 'content-type: application/json' -d '{}' -o /dev/null -w "%{http_code}\n"  # 400/422 not 404
kill %1
```
Expected: `/` and `/admin/dlq` return `200` (SPA), `/status` returns `OK`, `/api/auth/login` is reachable (not 404). (If `cargo run` can't bind/connect, document the commands; the api_router integration test from Plan 3a already proves routing.)

- [ ] **Step 3: Commit (docs note if anything adjusted)**

No code change expected. If the smoke revealed a fix, commit it with a clear message.

---

## Self-Review

**Spec coverage (design §5.4/§7 admin slice):** users + dlq api modules/types/hooks ✓ (T1); UsersPage + edit-scopes dialog ✓ (T2); DlqPage + replay ✓ (T3); `/admin/*` under `RequireAdmin` + admin mutation test ✓ (T4); end-to-end "app serves built SPA, no route collision on `/admin/dlq`" smoke ✓ (T5). Pagination/filtering intentionally deferred (design §10).

**Placeholder scan:** shadcn `add` has explicit manual fallbacks; all page/hook/api code is complete.

**Type consistency:** `UserWithScopes{id,email,scopes}`, `ScopeInfo{id,name,description}`, `DeadLetter{delivery_id,subscriber_name,event_type,aggregate_id,payload,last_error,attempts}` match the Rust DTOs (`UserWithScopes`, `ScopeInfo`, `DeadLetter`). Hook names (`useUsers`,`useScopes`,`useSetUserScopes`,`useDeadLetters`,`useReplayDeadLetter`) consistent across hooks/pages. API paths use the `/api`-relative form `apiFetch` prepends (e.g. `/users`, `/admin/dlq`). `RequireAdmin`/`AppLayout` from 3b reused.
