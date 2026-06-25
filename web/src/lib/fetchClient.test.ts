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
