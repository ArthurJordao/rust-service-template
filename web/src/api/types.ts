import type { components } from "@/api/schema";

export type AuthTokens = components["schemas"]["AuthTokens"];
export type Account = components["schemas"]["Account"];
export type UserWithScopes = components["schemas"]["UserWithScopes"];
export type ScopeInfo = components["schemas"]["ScopeRow"];   // backend type is ScopeRow
export type DeadLetter = components["schemas"]["DeadLetter"];
export type SentNotification = components["schemas"]["SentNotification"];
