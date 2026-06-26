import { ApiError } from "@/lib/fetchClient";

/// A " (ref: <cid>)" suffix for user-facing error messages, when the error carries
/// a correlation id the user can quote to support. Empty otherwise.
export function refSuffix(e: unknown): string {
  return e instanceof ApiError && e.cid ? ` (ref: ${e.cid})` : "";
}
