import { jwtDecode } from "jwt-decode";

interface AccessClaims {
  sub: string;
  email?: string;
  scopes?: string[];
  exp: number;
}

export function decodeAccessToken(token: string) {
  try {
    const c = jwtDecode<AccessClaims>(token);
    return { sub: c.sub, email: c.email, scopes: c.scopes ?? [], exp: c.exp };
  } catch {
    return null;
  }
}
