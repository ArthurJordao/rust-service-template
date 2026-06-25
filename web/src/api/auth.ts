import { apiFetch } from "@/lib/fetchClient";
import type { AuthTokens } from "@/api/types";

export const login = (email: string, password: string) =>
  apiFetch<AuthTokens>("/auth/login", { method: "POST", body: { email, password }, auth: false });

export const register = (email: string, password: string) =>
  apiFetch<AuthTokens>("/auth/register", { method: "POST", body: { email, password }, auth: false });

export const logout = (refresh_token: string, access_token: string | null) =>
  apiFetch<void>("/auth/logout", { method: "POST", body: { refresh_token, access_token }, auth: false });
