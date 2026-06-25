# Spec 3b: SPA Foundation (auth, fetch client, account) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold the `web/` React SPA and build its foundation: the typed fetch client with silent token refresh, the auth provider (access in memory + refresh in localStorage), TanStack Query data layer, route guards, app shell, and the `/login` + `/register` + account pages — with Vitest/MSW tests for the load-bearing logic.

**Architecture:** Vite + React 19 + TS + Tailwind + shadcn. A module-level `tokenStore` holds the access token so the framework-free `fetchClient` can read it; `fetchClient` does single-flight 401→refresh→retry. `AuthProvider` owns session state and silent-refreshes on boot. TanStack Query hooks wrap the `api/` modules. react-router v7 with `<RequireAuth>` guarding the app shell.

**Tech Stack:** Vite, React 19, TypeScript, Tailwind, shadcn/ui (Radix), react-router-dom v7, @tanstack/react-query, sonner, jwt-decode, lucide-react; Vitest + React Testing Library + jsdom + MSW.

## Global Constraints

- Depends on Spec 3a (backend `/api` routes, `/accounts/me`, DLQ). API base path is **`/api`**.
- Node ≥ 20 (this env: Node 26, npm 11). Package manager: **npm** (no pnpm/yarn).
- Access token lives **in memory only**; refresh token in `localStorage` key `rst:refresh`. The fetch client reads the access token from a module-level `tokenStore`, not React context.
- All API calls go through `fetchClient`; never call `fetch` directly in components/hooks.
- TypeScript `strict: true`. ESLint + Prettier clean. `npm run build` (tsc + vite build) must pass.
- Tests: Vitest + RTL + MSW. Run `npm test` (non-watch) — must pass.
- Work in `web/`. Commit messages prefixed `feat(web):` / `chore(web):` / `test(web):`.

---

### Task 1: Scaffold Vite + Tailwind + shadcn

**Files:**
- Create: `web/` (Vite React-TS scaffold), `web/vite.config.ts`, `web/tailwind.config.ts`, `web/postcss.config.js`, `web/src/index.css`, `web/components.json`, `web/.env.example`, `web/.eslintrc.cjs`, `web/.prettierrc`, `web/tsconfig*.json`

**Interfaces:**
- Produces: a building, lint-clean empty SPA with Tailwind + shadcn configured and `/api` dev proxy.

- [ ] **Step 1: Scaffold Vite React-TS**

From the repo root:
```bash
npm create vite@latest web -- --template react-ts
cd web && npm install
```

- [ ] **Step 2: Install runtime + dev deps**

In `web/`:
```bash
npm install react-router-dom @tanstack/react-query sonner jwt-decode lucide-react clsx tailwind-merge class-variance-authority
npm install -D tailwindcss postcss autoprefixer @types/node vitest jsdom @testing-library/react @testing-library/jest-dom @testing-library/user-event msw eslint prettier
npx tailwindcss init -p
```

- [ ] **Step 3: Configure Tailwind**

`web/tailwind.config.ts`:
```ts
import type { Config } from "tailwindcss";
export default {
  darkMode: ["class"],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: { extend: {} },
  plugins: [],
} satisfies Config;
```
Replace `web/src/index.css` with Tailwind directives + base layer:
```css
@tailwind base;
@tailwind components;
@tailwind utilities;
```

- [ ] **Step 4: Path alias + vite proxy**

`web/vite.config.ts`:
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
  server: {
    port: 5173,
    proxy: { "/api": { target: "http://localhost:8080", changeOrigin: true } },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
```
In `web/tsconfig.json` (or `tsconfig.app.json`) add under `compilerOptions`: `"baseUrl": ".", "paths": { "@/*": ["./src/*"] }`, and ensure `"strict": true`.

- [ ] **Step 5: Init shadcn + lib/utils**

```bash
npx shadcn@latest init -d
```
(If the CLI prompts despite `-d`, answer: style "new-york", base color "neutral", CSS variables yes. It writes `components.json` and `src/lib/utils.ts` with `cn()`.) If the CLI cannot run non-interactively, create `web/components.json` manually:
```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": { "config": "tailwind.config.ts", "css": "src/index.css", "baseColor": "neutral", "cssVariables": true },
  "aliases": { "components": "@/components", "utils": "@/lib/utils" }
}
```
and `web/src/lib/utils.ts`:
```ts
import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```

- [ ] **Step 6: `.env.example` + ESLint/Prettier**

`web/.env.example`:
```
VITE_API_BASE_URL=/api
```
`web/.prettierrc`: `{ "semi": true, "singleQuote": false }`
`web/.eslintrc.cjs`: a minimal config extending `eslint:recommended` + `plugin:@typescript-eslint/recommended` + react-hooks (use the config Vite's template generated if present; otherwise the standard typescript-eslint flat/legacy config).

- [ ] **Step 7: Verify build + lint**

In `web/`:
```bash
npm run build
npm run lint
```
Expected: both succeed (empty app builds).

- [ ] **Step 8: Commit**

```bash
git add web .gitignore
git commit -m "chore(web): scaffold Vite + React + TS + Tailwind + shadcn"
```

---

### Task 2: Token store + typed fetch client (401 silent refresh)

**Files:**
- Create: `web/src/auth/tokenStore.ts`, `web/src/lib/fetchClient.ts`
- Create: `web/src/test/setup.ts`, `web/src/lib/fetchClient.test.ts`

**Interfaces:**
- Produces:
  - `tokenStore`: `getAccessToken()`, `setAccessToken(t: string | null)`, `getRefreshToken()`, `setRefreshToken(t: string | null)` (refresh persisted to `localStorage`), `clear()`.
  - `class ApiError extends Error { status: number }`
  - `apiFetch<T>(path: string, opts?: { method?, body?, auth?: boolean }): Promise<T>` — JSON in/out, attaches bearer, single-flight 401→refresh→retry-once; `onAuthFailure` callback hook.
  - `setOnAuthFailure(fn: () => void)` — called when refresh fails (AuthProvider wires logout/redirect).

- [ ] **Step 1: Test setup (MSW)**

`web/src/test/setup.ts`:
```ts
import "@testing-library/jest-dom/vitest";
import { afterAll, afterEach, beforeAll } from "vitest";
import { setupServer } from "msw/node";

export const server = setupServer();
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());
```

- [ ] **Step 2: Write the failing test**

`web/src/lib/fetchClient.test.ts`:
```ts
import { http, HttpResponse } from "msw";
import { beforeEach, describe, expect, it } from "vitest";
import { server } from "@/test/setup";
import { apiFetch, ApiError } from "@/lib/fetchClient";
import { tokenStore } from "@/auth/tokenStore";

describe("apiFetch", () => {
  beforeEach(() => tokenStore.clear());

  it("attaches bearer and returns json", async () => {
    tokenStore.setAccessToken("at-1");
    server.use(http.get("/api/accounts/me", ({ request }) => {
      expect(request.headers.get("authorization")).toBe("Bearer at-1");
      return HttpResponse.json({ email: "a@b.c" });
    }));
    const res = await apiFetch<{ email: string }>("/accounts/me");
    expect(res.email).toBe("a@b.c");
  });

  it("on 401, refreshes once and retries", async () => {
    tokenStore.setAccessToken("stale");
    tokenStore.setRefreshToken("rt-1");
    let calls = 0;
    server.use(
      http.get("/api/accounts/me", ({ request }) => {
        const auth = request.headers.get("authorization");
        if (auth === "Bearer stale") return new HttpResponse(null, { status: 401 });
        return HttpResponse.json({ email: "ok@b.c" });
      }),
      http.post("/api/auth/refresh", () => {
        calls++;
        return HttpResponse.json({ access_token: "fresh", refresh_token: "rt-1", token_type: "Bearer", expires_in: 900 });
      }),
    );
    const res = await apiFetch<{ email: string }>("/accounts/me");
    expect(res.email).toBe("ok@b.c");
    expect(calls).toBe(1);
    expect(tokenStore.getAccessToken()).toBe("fresh");
  });

  it("throws ApiError with status on non-2xx (no refresh path)", async () => {
    server.use(http.get("/api/accounts/me", () => HttpResponse.json({ error: "nope" }, { status: 404 })));
    await expect(apiFetch("/accounts/me")).rejects.toMatchObject({ status: 404 });
    await expect(apiFetch("/accounts/me")).rejects.toBeInstanceOf(ApiError);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

In `web/`: `npm test -- run`
Expected: FAIL — modules not found.

- [ ] **Step 4: Implement tokenStore**

`web/src/auth/tokenStore.ts`:
```ts
const REFRESH_KEY = "rst:refresh";
let accessToken: string | null = null;

export const tokenStore = {
  getAccessToken: () => accessToken,
  setAccessToken: (t: string | null) => { accessToken = t; },
  getRefreshToken: () => localStorage.getItem(REFRESH_KEY),
  setRefreshToken: (t: string | null) => {
    if (t) localStorage.setItem(REFRESH_KEY, t);
    else localStorage.removeItem(REFRESH_KEY);
  },
  clear: () => {
    accessToken = null;
    localStorage.removeItem(REFRESH_KEY);
  },
};
```

- [ ] **Step 5: Implement fetchClient**

`web/src/lib/fetchClient.ts`:
```ts
import { tokenStore } from "@/auth/tokenStore";

const BASE = import.meta.env.VITE_API_BASE_URL ?? "/api";

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = "ApiError";
  }
}

let onAuthFailure: () => void = () => {};
export function setOnAuthFailure(fn: () => void) { onAuthFailure = fn; }

let refreshing: Promise<boolean> | null = null;

async function refreshAccessToken(): Promise<boolean> {
  const refresh = tokenStore.getRefreshToken();
  if (!refresh) return false;
  const res = await fetch(`${BASE}/auth/refresh`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ refresh_token: refresh }),
  });
  if (!res.ok) return false;
  const data = await res.json();
  tokenStore.setAccessToken(data.access_token);
  if (data.refresh_token) tokenStore.setRefreshToken(data.refresh_token);
  return true;
}

interface Opts {
  method?: string;
  body?: unknown;
  auth?: boolean; // default true
}

async function raw(path: string, opts: Opts): Promise<Response> {
  const headers: Record<string, string> = {};
  if (opts.body !== undefined) headers["content-type"] = "application/json";
  const token = tokenStore.getAccessToken();
  if (opts.auth !== false && token) headers["authorization"] = `Bearer ${token}`;
  return fetch(`${BASE}${path}`, {
    method: opts.method ?? "GET",
    headers,
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
  });
}

export async function apiFetch<T>(path: string, opts: Opts = {}): Promise<T> {
  let res = await raw(path, opts);

  if (res.status === 401 && opts.auth !== false) {
    // single-flight refresh
    refreshing ??= refreshAccessToken().finally(() => { refreshing = null; });
    const ok = await refreshing;
    if (ok) {
      res = await raw(path, opts);
    } else {
      onAuthFailure();
      throw new ApiError(401, "unauthorized");
    }
  }

  if (!res.ok) {
    let message = res.statusText;
    try { const j = await res.json(); message = j.error ?? message; } catch { /* ignore */ }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}
```

- [ ] **Step 6: Run test to verify it passes**

In `web/`: `npm test -- run`
Expected: PASS (3 fetchClient tests).

- [ ] **Step 7: Commit**

```bash
git add web/src
git commit -m "feat(web): token store + typed fetch client with single-flight 401 refresh"
```

---

### Task 3: jwt decode + query client + api modules/types

**Files:**
- Create: `web/src/lib/jwt.ts`, `web/src/lib/queryClient.ts`
- Create: `web/src/api/types.ts`, `web/src/api/auth.ts`, `web/src/api/accounts.ts`
- Create: `web/src/lib/jwt.test.ts`

**Interfaces:**
- Produces:
  - `decodeAccessToken(token: string): { sub: string; email?: string; scopes: string[]; exp: number } | null`
  - `queryClient` (TanStack `QueryClient`)
  - types: `AuthTokens`, `Account`; api fns `login`, `register`, `logout`, `getMe`.

- [ ] **Step 1: Write the failing jwt test**

`web/src/lib/jwt.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { decodeAccessToken } from "@/lib/jwt";

// header.payload.signature — payload base64url of {sub,email,scopes,exp,iat,jti,type}
function makeToken(payload: object): string {
  const b64 = (o: object) => btoa(JSON.stringify(o)).replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_");
  return `${b64({ alg: "RS256", typ: "JWT" })}.${b64(payload)}.sig`;
}

describe("decodeAccessToken", () => {
  it("extracts claims", () => {
    const t = makeToken({ sub: "user-7", email: "a@b.c", scopes: ["admin"], exp: 9999999999 });
    const c = decodeAccessToken(t)!;
    expect(c.sub).toBe("user-7");
    expect(c.email).toBe("a@b.c");
    expect(c.scopes).toEqual(["admin"]);
  });
  it("returns null on garbage", () => {
    expect(decodeAccessToken("not-a-jwt")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify fail**

`npm test -- run`  → FAIL (jwt not found).

- [ ] **Step 3: Implement jwt + queryClient**

`web/src/lib/jwt.ts`:
```ts
import { jwtDecode } from "jwt-decode";

interface AccessClaims {
  sub: string;
  email?: string;
  scopes?: string[];
  exp: number;
}

export function decodeAccessToken(token: string) {
  try {
    const c = jwtDecode<AccessClaims>(token);
    return { sub: c.sub, email: c.email, scopes: c.scopes ?? [], exp: c.exp };
  } catch {
    return null;
  }
}
```
`web/src/lib/queryClient.ts`:
```ts
import { QueryClient } from "@tanstack/react-query";
export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: 30_000 } },
});
```

- [ ] **Step 4: api types + modules**

`web/src/api/types.ts`:
```ts
export interface AuthTokens {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_in: number;
}
export interface Account {
  id: number;
  email: string;
  name: string;
  auth_user_id: number;
  created_at: string;
  created_by_cid: string;
}
```
`web/src/api/auth.ts`:
```ts
import { apiFetch } from "@/lib/fetchClient";
import type { AuthTokens } from "@/api/types";

export const login = (email: string, password: string) =>
  apiFetch<AuthTokens>("/auth/login", { method: "POST", body: { email, password }, auth: false });

export const register = (email: string, password: string) =>
  apiFetch<AuthTokens>("/auth/register", { method: "POST", body: { email, password }, auth: false });

export const logout = (refresh_token: string, access_token: string | null) =>
  apiFetch<void>("/auth/logout", { method: "POST", body: { refresh_token, access_token }, auth: false });
```
`web/src/api/accounts.ts`:
```ts
import { apiFetch } from "@/lib/fetchClient";
import type { Account } from "@/api/types";

export const getMe = () => apiFetch<Account>("/accounts/me");
```

- [ ] **Step 5: Run to verify pass**

`npm test -- run`  → PASS (jwt tests). `npm run build` → succeeds.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): jwt decode, query client, auth/accounts api modules + types"
```

---

### Task 4: AuthProvider + guards + boot refresh

**Files:**
- Create: `web/src/auth/AuthProvider.tsx`, `web/src/auth/useAuth.ts`, `web/src/auth/guards.tsx`
- Create: `web/src/auth/AuthProvider.test.tsx`

**Interfaces:**
- Consumes: `tokenStore`, `setOnAuthFailure`, `decodeAccessToken`, `login`/`register`/`logout` api.
- Produces:
  - `AuthProvider` (context) + `useAuth()` → `{ user: { email?: string; scopes: string[] } | null, isAdmin: boolean, status: "loading" | "ready", login(e,p), register(e,p), logout() }`
  - `<RequireAuth>` and `<RequireAdmin>` components.

- [ ] **Step 1: Write the failing test**

`web/src/auth/AuthProvider.test.tsx`:
```tsx
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, beforeEach } from "vitest";
import { server } from "@/test/setup";
import { AuthProvider, useAuth } from "@/auth/AuthProvider";
import { tokenStore } from "@/auth/tokenStore";

function Probe() {
  const { user, status, login } = useAuth();
  if (status === "loading") return <p>loading</p>;
  return (
    <div>
      <p>user:{user?.email ?? "none"}</p>
      <button onClick={() => login("a@b.c", "pw")}>login</button>
    </div>
  );
}

const TOKEN = `${btoa('{"alg":"RS256"}')}.${btoa('{"sub":"user-1","email":"a@b.c","scopes":[],"exp":9999999999}').replace(/=/g,"")}.s`;

describe("AuthProvider", () => {
  beforeEach(() => tokenStore.clear());

  it("boots to no-session when no refresh token", async () => {
    render(<AuthProvider><Probe /></AuthProvider>);
    await waitFor(() => expect(screen.getByText("user:none")).toBeInTheDocument());
  });

  it("login stores tokens and exposes the user", async () => {
    server.use(http.post("/api/auth/login", () =>
      HttpResponse.json({ access_token: TOKEN, refresh_token: "rt", token_type: "Bearer", expires_in: 900 })));
    render(<AuthProvider><Probe /></AuthProvider>);
    await waitFor(() => screen.getByText("user:none"));
    await userEvent.click(screen.getByText("login"));
    await waitFor(() => expect(screen.getByText("user:a@b.c")).toBeInTheDocument());
    expect(tokenStore.getRefreshToken()).toBe("rt");
  });
});
```

- [ ] **Step 2: Run to verify fail**

`npm test -- run` → FAIL.

- [ ] **Step 3: Implement AuthProvider + useAuth**

`web/src/auth/AuthProvider.tsx`:
```tsx
import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { tokenStore } from "@/auth/tokenStore";
import { setOnAuthFailure } from "@/lib/fetchClient";
import { decodeAccessToken } from "@/lib/jwt";
import * as authApi from "@/api/auth";

interface User { email?: string; scopes: string[]; }
interface AuthCtx {
  user: User | null;
  isAdmin: boolean;
  status: "loading" | "ready";
  login: (email: string, password: string) => Promise<void>;
  register: (email: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
}

const Ctx = createContext<AuthCtx | null>(null);

function userFromAccess(token: string | null): User | null {
  if (!token) return null;
  const c = decodeAccessToken(token);
  return c ? { email: c.email, scopes: c.scopes } : null;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [status, setStatus] = useState<"loading" | "ready">("loading");

  function applyTokens(access: string, refresh: string) {
    tokenStore.setAccessToken(access);
    tokenStore.setRefreshToken(refresh);
    setUser(userFromAccess(access));
  }

  useEffect(() => {
    setOnAuthFailure(() => { tokenStore.clear(); setUser(null); });
    const refresh = tokenStore.getRefreshToken();
    if (!refresh) { setStatus("ready"); return; }
    fetch(`${import.meta.env.VITE_API_BASE_URL ?? "/api"}/auth/refresh`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ refresh_token: refresh }),
    })
      .then((r) => (r.ok ? r.json() : Promise.reject()))
      .then((d) => applyTokens(d.access_token, d.refresh_token ?? refresh))
      .catch(() => { tokenStore.clear(); setUser(null); })
      .finally(() => setStatus("ready"));
  }, []);

  const value = useMemo<AuthCtx>(() => ({
    user,
    isAdmin: !!user?.scopes.includes("admin"),
    status,
    login: async (email, password) => {
      const t = await authApi.login(email, password);
      applyTokens(t.access_token, t.refresh_token);
    },
    register: async (email, password) => {
      const t = await authApi.register(email, password);
      applyTokens(t.access_token, t.refresh_token);
    },
    logout: async () => {
      const refresh = tokenStore.getRefreshToken();
      try { if (refresh) await authApi.logout(refresh, tokenStore.getAccessToken()); } catch { /* ignore */ }
      tokenStore.clear();
      setUser(null);
    },
  }), [user, status]);

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useAuth() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}
```
`web/src/auth/useAuth.ts`:
```ts
export { useAuth } from "@/auth/AuthProvider";
```

- [ ] **Step 4: Implement guards**

`web/src/auth/guards.tsx`:
```tsx
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
```

- [ ] **Step 5: Run to verify pass**

`npm test -- run` → PASS (AuthProvider tests).

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): AuthProvider (silent boot refresh) + useAuth + route guards"
```

---

### Task 5: shadcn components + AppLayout + routing + pages

**Files:**
- Add shadcn UI: button, input, label, card, sonner
- Create: `web/src/components/AppLayout.tsx`, `web/src/routes/LoginPage.tsx`, `web/src/routes/RegisterPage.tsx`, `web/src/routes/AccountPage.tsx`
- Modify: `web/src/App.tsx`, `web/src/main.tsx`
- Create: `web/src/api/hooks.ts`

**Interfaces:**
- Consumes: `useAuth`, `getMe`, shadcn components, `queryClient`.
- Produces: a runnable app — `/login`, `/register`, `/` (account), with nav + logout; `useMe()` hook.

- [ ] **Step 1: Add shadcn primitives**

In `web/`:
```bash
npx shadcn@latest add button input label card sonner
```
(If non-interactive add fails, create these components manually from the shadcn/ui "new-york" source — each is a small Radix-wrapping component under `src/components/ui/`.)

- [ ] **Step 2: `useMe` hook**

`web/src/api/hooks.ts`:
```ts
import { useQuery } from "@tanstack/react-query";
import { getMe } from "@/api/accounts";

export function useMe(enabled: boolean) {
  return useQuery({ queryKey: ["me"], queryFn: getMe, enabled });
}
```

- [ ] **Step 3: AppLayout (nav + logout)**

`web/src/components/AppLayout.tsx`:
```tsx
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
          <Link to="/" className="font-semibold">Account</Link>
          {isAdmin && <Link to="/admin/users">Users</Link>}
          {isAdmin && <Link to="/admin/dlq">DLQ</Link>}
        </div>
        <div className="flex items-center gap-3">
          <span className="text-sm text-muted-foreground">{user?.email}</span>
          <Button variant="outline" size="sm" onClick={async () => { await logout(); navigate("/login"); }}>
            Log out
          </Button>
        </div>
      </nav>
      <main className="p-6"><Outlet /></main>
    </div>
  );
}
```

- [ ] **Step 4: Login + Register pages**

`web/src/routes/LoginPage.tsx`:
```tsx
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card } from "@/components/ui/card";

export function LoginPage() {
  const { login } = useAuth();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    try { await login(email, password); navigate("/"); }
    catch { toast.error("Invalid credentials"); }
    finally { setBusy(false); }
  }

  return (
    <div className="mx-auto mt-24 max-w-sm">
      <Card className="p-6">
        <h1 className="mb-4 text-xl font-semibold">Sign in</h1>
        <form onSubmit={onSubmit} className="space-y-4">
          <div className="space-y-1"><Label htmlFor="email">Email</Label>
            <Input id="email" type="email" value={email} onChange={(e) => setEmail(e.target.value)} required /></div>
          <div className="space-y-1"><Label htmlFor="password">Password</Label>
            <Input id="password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} required /></div>
          <Button type="submit" className="w-full" disabled={busy}>Sign in</Button>
        </form>
        <p className="mt-4 text-sm">No account? <Link to="/register" className="underline">Register</Link></p>
      </Card>
    </div>
  );
}
```
`web/src/routes/RegisterPage.tsx`: identical structure but calls `register` and links back to `/login` (title "Create account", button "Register", error toast "Registration failed (email may be taken)").

- [ ] **Step 5: AccountPage**

`web/src/routes/AccountPage.tsx`:
```tsx
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
      {error && <p className="text-sm text-muted-foreground">No account yet for {user?.email}.</p>}
      {data && (
        <dl className="space-y-2 text-sm">
          <div><dt className="text-muted-foreground">Email</dt><dd>{data.email}</dd></div>
          <div><dt className="text-muted-foreground">Account ID</dt><dd>{data.id}</dd></div>
          <div><dt className="text-muted-foreground">Created</dt><dd>{new Date(data.created_at).toLocaleString()}</dd></div>
        </dl>
      )}
    </Card>
  );
}
```

- [ ] **Step 6: Routing + root providers**

`web/src/App.tsx`:
```tsx
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { RequireAuth } from "@/auth/guards";
import { AppLayout } from "@/components/AppLayout";
import { LoginPage } from "@/routes/LoginPage";
import { RegisterPage } from "@/routes/RegisterPage";
import { AccountPage } from "@/routes/AccountPage";

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route element={<RequireAuth />}>
          <Route element={<AppLayout />}>
            <Route path="/" element={<AccountPage />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
```
`web/src/main.tsx`:
```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "@/components/ui/sonner";
import { AuthProvider } from "@/auth/AuthProvider";
import { queryClient } from "@/lib/queryClient";
import { App } from "@/App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <App />
        <Toaster richColors />
      </AuthProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
```
(Admin routes `/admin/users` + `/admin/dlq` are added in Plan 3c.)

- [ ] **Step 7: Build + lint + test**

In `web/`: `npm run build && npm run lint && npm test -- run`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add web/src web/components.json
git commit -m "feat(web): app shell + routing + login/register/account pages"
```

---

## Self-Review

**Spec coverage (design §3/§5/§7/§8 for the 3b slice):** Vite/React/TS/Tailwind/shadcn scaffold ✓ (T1); tokenStore + fetchClient w/ single-flight 401 refresh ✓ (T2); jwt decode + queryClient + api modules/types ✓ (T3); AuthProvider (boot refresh) + useAuth + guards ✓ (T4); shell + routing + login/register/account + `useMe` ✓ (T5); MSW + Vitest tests for fetch/auth/jwt ✓ (T2/T3/T4). Admin pages deferred to 3c (per design §9).

**Placeholder scan:** the only "create it manually if the CLI is non-interactive" fallbacks (shadcn init/add) are explicit, with the exact `components.json`/`utils.ts` given; everything else is complete code.

**Type consistency:** `apiFetch<T>(path, opts)` signature consistent across api modules. `AuthTokens`/`Account` shapes match the Rust DTOs (`access_token`, `refresh_token`, `token_type`, `expires_in`; `Account{id,email,name,auth_user_id,created_at,created_by_cid}`). `useAuth()` shape (`user`, `isAdmin`, `status`, `login/register/logout`) consistent across guards/layout/pages. `decodeAccessToken` return shape consumed by `AuthProvider`.

**Known follow-up (3c):** `/admin/users` + `/admin/dlq` pages, `RequireAdmin` routes, users/dlq api modules + hooks, edit-scopes dialog, replay action.
