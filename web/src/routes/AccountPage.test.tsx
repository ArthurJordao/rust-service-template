import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { server } from "@/test/setup";
import { AuthProvider } from "@/auth/AuthProvider";
import { tokenStore } from "@/auth/tokenStore";
import { AccountPage } from "@/routes/AccountPage";

const TOKEN = `${btoa('{"alg":"RS256"}')}.${btoa(
  '{"sub":"user-1","email":"a@b.c","scopes":[],"exp":9999999999}',
).replace(/=/g, "")}.s`;

function mockSession() {
  server.use(
    http.post("/api/auth/refresh", () =>
      HttpResponse.json({ access_token: TOKEN, refresh_token: "rt", token_type: "Bearer", expires_in: 900 }),
    ),
    http.get("/api/accounts/me", () =>
      HttpResponse.json({ id: "acct-1", email: "a@b.c", created_at: new Date().toISOString() }),
    ),
  );
}

function renderPage() {
  tokenStore.setRefreshToken("rt");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthProvider>
        <AccountPage />
      </AuthProvider>
    </QueryClientProvider>,
  );
}

describe("AccountPage — MFA card", () => {
  beforeEach(() => tokenStore.clear());

  it("does not render a two-factor section when policy is off", async () => {
    mockSession();
    server.use(http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: false, policy: "off" })));
    renderPage();

    await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());
    expect(screen.queryByText(/two-factor authentication/i)).not.toBeInTheDocument();
  });

  it("shows an Enable control when policy is optional and MFA is not enabled", async () => {
    mockSession();
    server.use(http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: false, policy: "optional" })));
    renderPage();

    await waitFor(() => expect(screen.getByText(/two-factor authentication/i)).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /enable/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /regenerate/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /disable/i })).not.toBeInTheDocument();
  });

  it("shows Regenerate + Disable when policy is optional and MFA is enabled", async () => {
    mockSession();
    server.use(http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: true, policy: "optional" })));
    renderPage();

    await waitFor(() => expect(screen.getByRole("heading", { name: /two-factor authentication/i })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /regenerate/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /disable/i })).toBeInTheDocument();
  });

  it("hides Disable when policy is required, even if enabled", async () => {
    mockSession();
    server.use(http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: true, policy: "required" })));
    renderPage();

    await waitFor(() => expect(screen.getByRole("heading", { name: /two-factor authentication/i })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /regenerate/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /disable/i })).not.toBeInTheDocument();
  });

  it("enrolls via self-service (no bearer) and shows recovery codes without a token swap", async () => {
    mockSession();
    let setupBearer: string | null = null;
    let confirmBearer: string | null = null;
    server.use(
      http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: false, policy: "optional" })),
      http.post("/api/auth/mfa/setup", ({ request }) => {
        setupBearer = request.headers.get("authorization");
        return HttpResponse.json({ provisioning_uri: "otpauth://totp/example", secret: "SECRETKEY" });
      }),
      http.post("/api/auth/mfa/confirm", ({ request }) => {
        confirmBearer = request.headers.get("authorization");
        return HttpResponse.json({ recovery_codes: ["aaaaa-bbbbb", "ccccc-ddddd"], tokens: null });
      }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByRole("button", { name: /enable/i })).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: /enable/i }));

    await waitFor(() => expect(screen.getByText(/scan this with your authenticator/i)).toBeInTheDocument());
    expect(setupBearer).toBe(`Bearer ${TOKEN}`);

    await userEvent.type(screen.getByLabelText(/authentication or recovery code/i), "654321");
    await userEvent.click(screen.getByRole("button", { name: /confirm/i }));

    await waitFor(() => expect(screen.getByText(/save your recovery codes/i)).toBeInTheDocument());
    expect(confirmBearer).toBe(`Bearer ${TOKEN}`);
    expect(screen.getByText("aaaaa-bbbbb")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /done/i }));

    await waitFor(() => expect(screen.queryByText(/save your recovery codes/i)).not.toBeInTheDocument());
    expect(tokenStore.getAccessToken()).toBe(TOKEN); // unchanged: no token swap on self-service enroll
  });

  it("regenerates recovery codes after confirmation", async () => {
    mockSession();
    server.use(
      http.get("/api/auth/mfa", () => HttpResponse.json({ enabled: true, policy: "optional" })),
      http.post("/api/auth/mfa/recovery-codes", () => HttpResponse.json(["eeeee-fffff", "ggggg-hhhhh"])),
    );
    vi.spyOn(window, "confirm").mockReturnValue(true);
    renderPage();

    await waitFor(() => expect(screen.getByRole("button", { name: /regenerate/i })).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: /regenerate/i }));

    await waitFor(() => expect(screen.getByText(/save your recovery codes/i)).toBeInTheDocument());
    expect(screen.getByText("eeeee-fffff")).toBeInTheDocument();
  });

  it("disables MFA after confirmation and refreshes status", async () => {
    mockSession();
    let disableCalled = false;
    server.use(
      http.get("/api/auth/mfa", () => {
        if (disableCalled) return HttpResponse.json({ enabled: false, policy: "optional" });
        return HttpResponse.json({ enabled: true, policy: "optional" });
      }),
      http.delete("/api/auth/mfa", () => {
        disableCalled = true;
        return new HttpResponse(null, { status: 204 });
      }),
    );
    vi.spyOn(window, "confirm").mockReturnValue(true);
    renderPage();

    await waitFor(() => expect(screen.getByRole("button", { name: /disable/i })).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: /disable/i }));

    await waitFor(() => expect(screen.getByRole("button", { name: /enable/i })).toBeInTheDocument());
  });
});
