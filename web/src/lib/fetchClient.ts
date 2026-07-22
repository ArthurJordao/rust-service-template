import { tokenStore } from "@/auth/tokenStore";
import { newSegment } from "@/lib/cid";
import type { paths } from "@/api/schema";

// Expand OpenAPI path params ({id}) into `${string}` so interpolated calls type-check,
// while exact (param-free) paths still reject typos.
type ExpandPath<T extends string> =
  T extends `${infer Head}{${string}}${infer Tail}`
    ? `${Head}${string}${ExpandPath<Tail>}`
    : T;
export type ApiPath = ExpandPath<keyof paths & string>;

const BASE = import.meta.env.VITE_API_BASE_URL ?? "/api";

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

let onAuthFailure: () => void = () => {};
export function setOnAuthFailure(fn: () => void) { onAuthFailure = fn; }

let refreshing: Promise<boolean> | null = null;

async function refreshAccessToken(): Promise<boolean> {
  const res = await fetch(`${BASE}/auth/refresh`, { method: "POST", credentials: "include" });
  if (!res.ok) return false;
  const data = await res.json();
  tokenStore.setAccessToken(data.access_token);
  return true;
}

interface Opts {
  method?: string;
  body?: unknown;
  auth?: boolean; // default true
  bearer?: string; // explicit token (e.g. an mfa_token); skips access-token + refresh
}

async function raw(path: string, opts: Opts, cid: string): Promise<Response> {
  const headers: Record<string, string> = { "x-correlation-id": cid };
  if (opts.body !== undefined) headers["content-type"] = "application/json";
  if (opts.bearer) {
    headers["authorization"] = `Bearer ${opts.bearer}`;
  } else if (opts.auth !== false) {
    const token = tokenStore.getAccessToken();
    if (token) headers["authorization"] = `Bearer ${token}`;
  }
  return fetch(`${BASE}${path}`, {
    method: opts.method ?? "GET",
    headers,
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    credentials: "include",
  });
}

export async function apiFetch<T>(path: ApiPath, opts: Opts = {}): Promise<T> {
  const cid = newSegment();
  let res = await raw(path, opts, cid);

  if (res.status === 401 && opts.auth !== false && !opts.bearer) {
    // single-flight refresh
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
