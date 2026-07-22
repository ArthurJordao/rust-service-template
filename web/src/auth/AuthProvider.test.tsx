import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, beforeEach } from "vitest";
import { server } from "@/test/setup";
import { AuthProvider, useAuth } from "@/auth/AuthProvider";
import { tokenStore } from "@/auth/tokenStore";

function Probe() {
  const { user, status, login } = useAuth();
  if (status === "loading") return <p>loading</p>;
  return (
    <div>
      <p>user:{user?.email ?? "none"}</p>
      <button onClick={() => login("a@b.c", "pw")}>login</button>
    </div>
  );
}

const TOKEN = `${btoa('{"alg":"RS256"}')}.${btoa('{"sub":"user-1","email":"a@b.c","scopes":[],"exp":9999999999}').replace(/=/g,"")}.s`;

describe("AuthProvider", () => {
  beforeEach(() => tokenStore.clear());

  it("boots to no-session when the refresh cookie is absent (refresh 401s)", async () => {
    server.use(http.post("/api/auth/refresh", () => new HttpResponse(null, { status: 401 })));
    render(<AuthProvider><Probe /></AuthProvider>);
    await waitFor(() => expect(screen.getByText("user:none")).toBeInTheDocument());
  });

  it("bootstraps a session when the refresh cookie is present", async () => {
    server.use(
      http.post("/api/auth/refresh", () =>
        HttpResponse.json({ access_token: TOKEN, token_type: "Bearer", expires_in: 900 }),
      ),
    );
    render(<AuthProvider><Probe /></AuthProvider>);
    await waitFor(() => expect(screen.getByText("user:a@b.c")).toBeInTheDocument());
  });

  it("login stores the access token in memory and exposes the user", async () => {
    server.use(
      http.post("/api/auth/refresh", () => new HttpResponse(null, { status: 401 })),
      http.post("/api/auth/login", () =>
        HttpResponse.json({ status: "authenticated", tokens: { access_token: TOKEN, token_type: "Bearer", expires_in: 900 } }),
      ),
    );
    render(<AuthProvider><Probe /></AuthProvider>);
    await waitFor(() => screen.getByText("user:none"));
    await userEvent.click(screen.getByText("login"));
    await waitFor(() => expect(screen.getByText("user:a@b.c")).toBeInTheDocument());
    expect(tokenStore.getAccessToken()).toBe(TOKEN);
  });
});
