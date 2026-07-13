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

  it("uses the bearer override and does not refresh on 401", async () => {
    let seenAuth: string | null = null;
    let calls = 0;
    server.use(
      http.post("/api/auth/mfa/verify", ({ request }) => {
        calls++;
        seenAuth = request.headers.get("authorization");
        return HttpResponse.json({ error: "nope" }, { status: 401 });
      }),
    );
    await expect(
      apiFetch("/auth/mfa/verify", { method: "POST", body: { code: "x" }, bearer: "MFA_TOKEN" }),
    ).rejects.toMatchObject({ status: 401 });
    expect(seenAuth).toBe("Bearer MFA_TOKEN");
    expect(calls).toBe(1); // no refresh-retry
  });
});
