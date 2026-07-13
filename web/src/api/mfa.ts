import { apiFetch } from "@/lib/fetchClient";
import type { components } from "@/api/schema";

type MfaStatus = components["schemas"]["MfaStatusResponse"];
type MfaSetupResponse = components["schemas"]["MfaSetupResponse"];
type MfaConfirmResponse = components["schemas"]["MfaConfirmResponse"];
type AuthTokens = components["schemas"]["AuthTokens"];

export const mfaStatus = () => apiFetch<MfaStatus>("/auth/mfa");

export const mfaSetup = (bearer?: string) =>
  apiFetch<MfaSetupResponse>("/auth/mfa/setup", { method: "POST", bearer });

export const mfaConfirm = (code: string, bearer?: string) =>
  apiFetch<MfaConfirmResponse>("/auth/mfa/confirm", { method: "POST", body: { code }, bearer });

export const mfaVerify = (code: string, mfaToken: string) =>
  apiFetch<AuthTokens>("/auth/mfa/verify", { method: "POST", body: { code }, bearer: mfaToken });

export const regenRecoveryCodes = () =>
  apiFetch<string[]>("/auth/mfa/recovery-codes", { method: "POST" });

export const disableMfa = () => apiFetch<void>("/auth/mfa", { method: "DELETE" });

export const adminResetMfa = (id: number) =>
  apiFetch<void>(`/admin/users/${id}/mfa/reset`, { method: "POST" });
