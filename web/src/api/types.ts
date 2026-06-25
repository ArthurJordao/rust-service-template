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
