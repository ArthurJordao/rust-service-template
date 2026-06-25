import { apiFetch } from "@/lib/fetchClient";
import type { Account } from "@/api/types";

export const getMe = () => apiFetch<Account>("/accounts/me");
