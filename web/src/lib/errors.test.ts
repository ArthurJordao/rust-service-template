import { describe, expect, it } from "vitest";
import { ApiError } from "@/lib/fetchClient";
import { refSuffix } from "@/lib/errors";

describe("refSuffix", () => {
  it("includes the cid for an ApiError with one", () => {
    expect(refSuffix(new ApiError(500, "boom", "root.ab12cd"))).toBe(" (ref: root.ab12cd)");
  });
  it("is empty for a cid-less ApiError or a plain error", () => {
    expect(refSuffix(new ApiError(400, "bad"))).toBe("");
    expect(refSuffix(new Error("x"))).toBe("");
    expect(refSuffix(undefined)).toBe("");
  });
});
