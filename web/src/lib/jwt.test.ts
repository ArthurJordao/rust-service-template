import { describe, expect, it } from "vitest";
import { decodeAccessToken } from "@/lib/jwt";

// header.payload.signature — payload base64url of {sub,email,scopes,exp,iat,jti,type}
function makeToken(payload: object): string {
  const b64 = (o: object) => btoa(JSON.stringify(o)).replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_");
  return `${b64({ alg: "RS256", typ: "JWT" })}.${b64(payload)}.sig`;
}

describe("decodeAccessToken", () => {
  it("extracts claims", () => {
    const t = makeToken({ sub: "user-7", email: "a@b.c", scopes: ["admin"], exp: 9999999999 });
    const c = decodeAccessToken(t)!;
    expect(c.sub).toBe("user-7");
    expect(c.email).toBe("a@b.c");
    expect(c.scopes).toEqual(["admin"]);
  });
  it("returns null on garbage", () => {
    expect(decodeAccessToken("not-a-jwt")).toBeNull();
  });
});
