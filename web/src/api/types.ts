export interface AuthTokens {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_in: number;
}
export interface Account {
  id: number;
  email: string;
  name: string;
  auth_user_id: number;
  created_at: string;
  created_by_cid: string;
}
export interface UserWithScopes {
  id: number;
  email: string;
  scopes: string[];
}
export interface ScopeInfo {
  id: number;
  name: string;
  description: string;
}
export interface DeadLetter {
  delivery_id: number;
  subscriber_name: string;
  event_type: string;
  aggregate_id: string;
  payload: unknown;
  last_error: string | null;
  attempts: number;
}
