import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { server } from "@/test/setup";
import { NotificationsPage } from "@/routes/admin/NotificationsPage";

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><NotificationsPage /></QueryClientProvider>);
}

describe("NotificationsPage", () => {
  it("renders notification rows", async () => {
    server.use(
      http.get("/api/notifications", () =>
        HttpResponse.json([
          {
            id: 1, source_event_id: 5, template: "welcome", subject: "Welcome",
            channel: "email", recipient: "a@b.c", body: "hello there",
            created_at: "2026-07-12T10:00:00Z", created_by_cid: "root.ab",
          },
        ]),
      ),
    );

    renderPage();
    await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());
    expect(screen.getByText("Welcome")).toBeInTheDocument();
  });

  it("shows empty state", async () => {
    server.use(http.get("/api/notifications", () => HttpResponse.json([])));
    renderPage();
    await waitFor(() => expect(screen.getByText(/no notifications/i)).toBeInTheDocument());
  });
});
