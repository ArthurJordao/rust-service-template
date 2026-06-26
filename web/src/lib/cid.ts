/// A short correlation-id segment (6 hex chars), mirroring the backend's new_segment().
export function newSegment(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 6);
}
