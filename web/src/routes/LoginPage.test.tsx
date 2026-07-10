import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, beforeEach } from "vitest";
import { server } from "@/test/setup";
import { AuthProvider } from "@/auth/AuthProvider";
import { tokenStore } from "@/auth/tokenStore";
import { LoginPage } from "@/routes/LoginPage";

const FAKE_JWT = `${btoa('{"alg":"RS256"}')}.${btoa(
  '{"sub":"user-1","email":"a@b.c","scopes":[],"exp":9999999999}',
).replace(/=/g, "")}.s`;

function renderPage() {
  return render(
    <AuthProvider>
      <MemoryRouter initialEntries={["/login"]}>
        <Routes>
          <Route path="/login" element={<LoginPage />} />
          <Route path="/" element={<p>home</p>} />
        </Routes>
      </MemoryRouter>
    </AuthProvider>,
  );
}

async function fillAndSubmitPassword() {
  await userEvent.type(screen.getByLabelText(/email/i), "a@b.c");
  await userEvent.type(screen.getByLabelText(/^password$/i), "pw");
  await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
}

describe("LoginPage", () => {
  beforeEach(() => tokenStore.clear());

  it("completes a verify-step login", async () => {
    server.use(
      http.post("/api/auth/login", () =>
        HttpResponse.json({
          status: "mfa_required",
          purpose: "verify",
          mfa_token: "MFA",
          factor_types: ["totp"],
        }),
      ),
      http.post("/api/auth/mfa/verify", () =>
        HttpResponse.json({
          access_token: FAKE_JWT,
          refresh_token: "rt",
          token_type: "Bearer",
          expires_in: 900,
        }),
      ),
    );

    renderPage();
    await fillAndSubmitPassword();

    await waitFor(() => expect(screen.getByLabelText(/authentication or recovery code/i)).toBeInTheDocument());
    await userEvent.type(screen.getByLabelText(/authentication or recovery code/i), "123456");
    await userEvent.click(screen.getByRole("button", { name: /verify/i }));

    await waitFor(() => expect(screen.getByText("home")).toBeInTheDocument());
    expect(tokenStore.getRefreshToken()).toBe("rt");
    expect(tokenStore.getAccessToken()).toBe(FAKE_JWT);
  });

  it("completes an enroll-step login", async () => {
    server.use(
      http.post("/api/auth/login", () =>
        HttpResponse.json({
          status: "mfa_required",
          purpose: "enroll",
          mfa_token: "MFA",
          factor_types: ["totp"],
        }),
      ),
      http.post("/api/auth/mfa/setup", () =>
        HttpResponse.json({
          provisioning_uri: "otpauth://totp/example",
          secret: "SECRETKEY",
        }),
      ),
      http.post("/api/auth/mfa/confirm", () =>
        HttpResponse.json({
          recovery_codes: ["aaaaa-bbbbb", "ccccc-ddddd"],
          tokens: {
            access_token: FAKE_JWT,
            refresh_token: "rt2",
            token_type: "Bearer",
            expires_in: 900,
          },
        }),
      ),
    );

    renderPage();
    await fillAndSubmitPassword();

    await waitFor(() => expect(screen.getByText(/set up two-factor authentication/i)).toBeInTheDocument());
    await userEvent.type(screen.getByLabelText(/authentication or recovery code/i), "654321");
    await userEvent.click(screen.getByRole("button", { name: /confirm/i }));

    await waitFor(() => expect(screen.getByText(/save your recovery codes/i)).toBeInTheDocument());
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /done/i }));

    await waitFor(() => expect(screen.getByText("home")).toBeInTheDocument());
    expect(tokenStore.getRefreshToken()).toBe("rt2");
    expect(tokenStore.getAccessToken()).toBe(FAKE_JWT);
  });

  it("resets to the password step when the mfa_token has expired", async () => {
    server.use(
      http.post("/api/auth/login", () =>
        HttpResponse.json({
          status: "mfa_required",
          purpose: "verify",
          mfa_token: "MFA",
          factor_types: ["totp"],
        }),
      ),
      http.post("/api/auth/mfa/verify", () => HttpResponse.json({ error: "expired" }, { status: 401 })),
    );

    renderPage();
    await fillAndSubmitPassword();

    await waitFor(() => expect(screen.getByLabelText(/authentication or recovery code/i)).toBeInTheDocument());
    await userEvent.type(screen.getByLabelText(/authentication or recovery code/i), "123456");
    await userEvent.click(screen.getByRole("button", { name: /verify/i }));

    await waitFor(() => expect(screen.getByRole("heading", { name: /sign in/i })).toBeInTheDocument());
    expect(screen.getByLabelText(/email/i)).toBeInTheDocument();
    expect(tokenStore.getAccessToken()).toBeNull();
  });
});
