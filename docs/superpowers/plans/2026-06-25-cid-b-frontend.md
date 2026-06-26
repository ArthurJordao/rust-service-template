# Correlation-ID + Logging — Plan B (frontend) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the SPA originate a correlation id on every API call (`X-Correlation-Id`), keep it stable across a 401-refresh retry, capture the server's echoed cid, and surface it to the user as a support reference on errors.

**Architecture:** A `newSegment()` helper mirrors the backend's 6-char segment; `apiFetch` mints one cid per call and sends it, reusing it on the retry; `ApiError` carries the response cid; error toasts append `(ref: <cid>)`.

**Tech Stack:** TypeScript, the existing custom `fetchClient` + TanStack Query SPA, Vitest + MSW.

## Global Constraints

- Depends on Plan A (backend appends + echoes the cid). Spec: `docs/superpowers/specs/2026-06-25-correlation-id-logging-design.md` §4.
- Header name `X-Correlation-Id` (case-insensitive). One cid per `apiFetch` call; the 401→refresh→retry reuses the **same** cid; the internal `/auth/refresh` call gets its own.
- Keep the custom `fetchClient` (single-flight 401 refresh, `ApiError`); this only *adds* the cid header + capture.
- TS `strict`. `npm run build` + `npm run lint` + `npm test -- run` must pass. Run npm from `web/`. Commit prefix `feat(web):` / `test(web):`.

---

### Task 1: cid origination + capture in `fetchClient`

**Files:** Create `web/src/lib/cid.ts`; modify `web/src/lib/fetchClient.ts`; extend `web/src/lib/fetchClient.test.ts`.

**Interfaces:**
- Produces: `newSegment(): string` (6 chars); `apiFetch` sends `X-Correlation-Id` and reuses it across the retry; `ApiError` gains `cid?: string` populated from the response header on non-2xx.

- [ ] **Step 1: Write `newSegment`**

`web/src/lib/cid.ts`:
```ts
/// A short correlation-id segment (6 hex chars), mirroring the backend's new_segment().
export function newSegment(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 6);
}
```

- [ ] **Step 2: Write the failing tests**

Append to `web/src/lib/fetchClient.test.ts`:
```ts
it("sends an X-Correlation-Id header", async () => {
  let seen: string | null = null;
  server.use(
    http.get("/api/accounts/me", ({ request }) => {
      seen = request.headers.get("x-correlation-id");
      return HttpResponse.json({ email: "a@b.c" });
    }),
  );
  await apiFetch("/accounts/me");
  expect(seen).toBeTruthy();
  expect(seen!.length).toBeGreaterThanOrEqual(6);
});

it("reuses the same cid across a 401 refresh+retry", async () => {
  tokenStore.setAccessToken("stale");
  tokenStore.setRefreshToken("rt-1");
  const cids: string[] = [];
  server.use(
    http.get("/api/accounts/me", ({ request }) => {
      cids.push(request.headers.get("x-correlation-id") ?? "");
      const auth = request.headers.get("authorization");
      if (auth === "Bearer stale") return new HttpResponse(null, { status: 401 });
      return HttpResponse.json({ email: "ok@b.c" });
    }),
    http.post("/api/auth/refresh", () =>
      HttpResponse.json({ access_token: "fresh", refresh_token: "rt-1", token_type: "Bearer", expires_in: 900 })),
  );
  await apiFetch("/accounts/me");
  expect(cids).toHaveLength(2);
  expect(cids[0]).toBe(cids[1]); // same cid on the retry
});

it("populates ApiError.cid from the response header", async () => {
  server.use(
    http.get("/api/accounts/me", () =>
      HttpResponse.json({ error: "nope" }, { status: 404, headers: { "x-correlation-id": "root.ab12cd" } })),
  );
  await expect(apiFetch("/accounts/me")).rejects.toMatchObject({ status: 404, cid: "root.ab12cd" });
});
```

- [ ] **Step 3: Run to verify they fail**

In `web/`: `npm test -- run`
Expected: FAIL — no header sent / `cid` undefined.

- [ ] **Step 4: Implement in `fetchClient.ts`**

Add the import:
```ts
import { newSegment } from "@/lib/cid";
```
Extend `ApiError` with `cid`:
```ts
export class ApiError extends Error {
  status: number;
  cid?: string;
  constructor(status: number, message: string, cid?: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.cid = cid;
  }
}
```
Thread the cid through `raw` (so the initial call and the retry share it). Change `raw`'s signature to accept the cid and set the header:
```ts
async function raw(path: string, opts: Opts, cid: string): Promise<Response> {
  const headers: Record<string, string> = { "x-correlation-id": cid };
  if (opts.body !== undefined) headers["content-type"] = "application/json";
  const token = tokenStore.getAccessToken();
  if (opts.auth !== false && token) headers["authorization"] = `Bearer ${token}`;
  return fetch(`${BASE}${path}`, {
    method: opts.method ?? "GET",
    headers,
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
  });
}
```
Update `apiFetch` to mint the cid once and capture it on errors:
```ts
export async function apiFetch<T>(path: string, opts: Opts = {}): Promise<T> {
  const cid = newSegment();
  let res = await raw(path, opts, cid);

  if (res.status === 401 && opts.auth !== false) {
    refreshing ??= refreshAccessToken().finally(() => { refreshing = null; });
    const ok = await refreshing;
    if (ok) {
      res = await raw(path, opts, cid); // same cid on retry
    } else {
      onAuthFailure();
      throw new ApiError(401, "unauthorized", res.headers.get("x-correlation-id") ?? undefined);
    }
  }

  if (!res.ok) {
    let message = res.statusText;
    try { const j = await res.json(); message = j.error ?? message; } catch { /* ignore */ }
    throw new ApiError(res.status, message, res.headers.get("x-correlation-id") ?? undefined);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}
```

- [ ] **Step 5: Run to verify they pass**

In `web/`: `npm test -- run`
Expected: PASS (all fetchClient tests, including the prior ones).

- [ ] **Step 6: build + lint + commit**
```
npm run build && npm run lint
git add web/src
git commit -m "feat(web): send X-Correlation-Id per request (stable across retry) + capture on ApiError"
```

---

### Task 2: Surface the cid as a support reference on errors

**Files:** Create `web/src/lib/errors.ts`; modify `web/src/routes/LoginPage.tsx`, `web/src/routes/RegisterPage.tsx`, `web/src/api/hooks.ts`; test `web/src/lib/errors.test.ts`.

**Interfaces:**
- Produces: `refSuffix(e: unknown): string` → `" (ref: <cid>)"` when `e` is an `ApiError` with a cid, else `""`. Used in error toasts.

- [ ] **Step 1: Write the failing test**

`web/src/lib/errors.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { ApiError } from "@/lib/fetchClient";
import { refSuffix } from "@/lib/errors";

describe("refSuffix", () => {
  it("includes the cid for an ApiError with one", () => {
    expect(refSuffix(new ApiError(500, "boom", "root.ab12cd"))).toBe(" (ref: root.ab12cd)");
  });
  it("is empty for a cid-less ApiError or a plain error", () => {
    expect(refSuffix(new ApiError(400, "bad"))).toBe("");
    expect(refSuffix(new Error("x"))).toBe("");
    expect(refSuffix(undefined)).toBe("");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

In `web/`: `npm test -- run`
Expected: FAIL — `refSuffix` not found.

- [ ] **Step 3: Implement `refSuffix`**

`web/src/lib/errors.ts`:
```ts
import { ApiError } from "@/lib/fetchClient";

/// A " (ref: <cid>)" suffix for user-facing error messages, when the error carries
/// a correlation id the user can quote to support. Empty otherwise.
export function refSuffix(e: unknown): string {
  return e instanceof ApiError && e.cid ? ` (ref: ${e.cid})` : "";
}
```

- [ ] **Step 4: Use it in the error toasts**

- `web/src/routes/LoginPage.tsx`: import `refSuffix` and change the catch:
  ```ts
  catch (e) { toast.error("Invalid credentials" + refSuffix(e)); }
  ```
- `web/src/routes/RegisterPage.tsx`: similarly:
  ```ts
  catch (e) { toast.error("Registration failed (email may be taken)" + refSuffix(e)); }
  ```
- `web/src/api/hooks.ts`: in `useSetUserScopes` and `useReplayDeadLetter` `onError`, change to receive the error and append the ref:
  ```ts
  onError: (e) => toast.error("Failed to update scopes" + refSuffix(e)),
  ```
  ```ts
  onError: (e) => toast.error("Replay failed" + refSuffix(e)),
  ```
  (add `import { refSuffix } from "@/lib/errors";` to `hooks.ts`.)

- [ ] **Step 5: Run + gate**

In `web/`: `npm test -- run && npm run build && npm run lint`
Expected: all pass.

- [ ] **Step 6: Commit**
```
git add web/src
git commit -m "feat(web): show correlation-id as a support reference on error toasts"
```

---

## Self-Review

**Spec coverage (design §4):** `newSegment` ✓ (T1 S1); per-request `X-Correlation-Id` + same-cid retry ✓ (T1 S4 + test); `ApiError.cid` from the response header ✓ (T1); error-toast reference via `refSuffix` ✓ (T2). The optional dev-only `console.debug` is omitted (YAGNI; not asserted by the spec).

**Placeholder scan:** none — complete code per step.

**Type consistency:** `newSegment(): string` consumed by `apiFetch`. `ApiError(status, message, cid?)` constructor consistent across `fetchClient.ts` and `errors.ts`/tests. `refSuffix(unknown): string` consumed by both pages + both mutation hooks. The header string `"x-correlation-id"` matches the backend's `CORRELATION_ID_HEADER`.

**Dependency:** Plan B's response-cid capture is meaningful only once Plan A echoes the appended cid (it does today — the middleware already echoes — but Plan A makes that cid hierarchical).
