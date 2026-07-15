import { apiFetch } from "@/lib/fetchClient";
import type { SentNotification } from "@/api/types";

export const listNotifications = () => apiFetch<SentNotification[]>("/notifications");
