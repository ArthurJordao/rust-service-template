import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { server } from "@/test/setup";
import { UsersPage } from "@/routes/admin/UsersPage";

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><UsersPage /></QueryClientProvider>);
}

describe("UsersPage", () => {
  it("lists users and saves edited scopes", async () => {
    let putBody: unknown = null;
    server.use(
      http.get("/api/users", () => HttpResponse.json([{ id: 1, email: "a@b.c", scopes: ["read:accounts:own"] }])),
      http.get("/api/scopes", () => HttpResponse.json([
        { id: 1, name: "admin", description: "all" },
        { id: 2, name: "read:accounts:own", description: "own" },
      ])),
      http.put("/api/users/1/scopes", async ({ request }) => { putBody = await request.json(); return new HttpResponse(null, { status: 204 }); }),
    );

    renderPage();
    await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());

    await userEvent.click(screen.getByText("Edit scopes"));
    await waitFor(() => screen.getByText("Scopes for a@b.c"));
    await userEvent.click(screen.getByRole("checkbox", { name: /admin/i }));   // check admin
    await userEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(putBody).toEqual({ scopes: expect.arrayContaining(["read:accounts:own", "admin"]) }));
  });

  it("resets a user's MFA after confirmation", async () => {
    let resetCalled = false;
    server.use(
      http.get("/api/users", () => HttpResponse.json([{ id: 1, email: "a@b.c", scopes: ["read:accounts:own"] }])),
      http.get("/api/scopes", () => HttpResponse.json([])),
      http.post("/api/admin/users/1/mfa/reset", () => { resetCalled = true; return new HttpResponse(null, { status: 204 }); }),
    );

    renderPage();
    await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());

    await userEvent.click(screen.getByText("Reset MFA"));
    await waitFor(() => screen.getByText("Reset this user's MFA?"));
    await userEvent.click(screen.getByRole("button", { name: "Reset" }));

    await waitFor(() => expect(resetCalled).toBe(true));
  });
});
