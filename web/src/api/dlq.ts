import { apiFetch } from "@/lib/fetchClient";
import type { DeadLetter } from "@/api/types";

export const listDeadLetters = () => apiFetch<DeadLetter[]>("/admin/dlq");
export const replayDeadLetter = (deliveryId: number) =>
  apiFetch<{ replayed: boolean }>(`/admin/dlq/${deliveryId}/replay`, { method: "POST" });
