import { apiFetch } from "@/lib/fetchClient";
import type { AuthTokens } from "@/api/types";
import type { components } from "@/api/schema";

type LoginResponse = components["schemas"]["LoginResponse"];

export const login = (email: string, password: string) =>
  apiFetch<LoginResponse>("/auth/login", { method: "POST", body: { email, password }, auth: false });

export const register = (email: string, password: string) =>
  apiFetch<AuthTokens>("/auth/register", { method: "POST", body: { email, password }, auth: false });

export const logout = (refresh_token: string, access_token: string | null) =>
  apiFetch<void>("/auth/logout", { method: "POST", body: { refresh_token, access_token }, auth: false });
