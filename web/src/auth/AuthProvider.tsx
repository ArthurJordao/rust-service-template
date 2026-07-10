import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import type { components } from "@/api/schema";
import { tokenStore } from "@/auth/tokenStore";
import { setOnAuthFailure } from "@/lib/fetchClient";
import { decodeAccessToken } from "@/lib/jwt";
import * as authApi from "@/api/auth";

type AuthTokens = components["schemas"]["AuthTokens"];
export interface MfaChallenge { mfa_token: string; purpose: string; factor_types: string[]; }

interface User { email?: string; scopes: string[]; }
interface AuthCtx {
  user: User | null;
  isAdmin: boolean;
  status: "loading" | "ready";
  login: (email: string, password: string) => Promise<MfaChallenge | null>;
  applySession: (tokens: AuthTokens) => void;
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
  const [status, setStatus] = useState<"loading" | "ready">(
    () => (tokenStore.getRefreshToken() ? "loading" : "ready"),
  );

  function applyTokens(access: string, refresh: string) {
    tokenStore.setAccessToken(access);
    tokenStore.setRefreshToken(refresh);
    setUser(userFromAccess(access));
  }

  useEffect(() => {
    setOnAuthFailure(() => { tokenStore.clear(); setUser(null); });
    const refresh = tokenStore.getRefreshToken();
    if (!refresh) return;
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
      const res = await authApi.login(email, password);
      if (res.status === "authenticated") {
        applyTokens(res.tokens.access_token, res.tokens.refresh_token);
        return null;
      }
      return { mfa_token: res.mfa_token, purpose: res.purpose, factor_types: res.factor_types };
    },
    applySession: (tokens) => applyTokens(tokens.access_token, tokens.refresh_token),
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
