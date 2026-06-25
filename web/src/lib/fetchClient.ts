import { tokenStore } from "@/auth/tokenStore";

const BASE = import.meta.env.VITE_API_BASE_URL ?? "/api";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
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
