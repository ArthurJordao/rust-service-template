import { apiFetch } from "@/lib/fetchClient";
import type { ScopeInfo, UserWithScopes } from "@/api/types";

export const listUsers = () => apiFetch<UserWithScopes[]>("/users");
export const listScopes = () => apiFetch<ScopeInfo[]>("/scopes");
export const setUserScopes = (id: number, scopes: string[]) =>
  apiFetch<void>(`/users/${id}/scopes`, { method: "PUT", body: { scopes } });
