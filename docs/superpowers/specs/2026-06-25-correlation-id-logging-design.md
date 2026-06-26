# Correlation IDs + Structured Logging — Design

**Date:** 2026-06-25
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Make a hierarchical correlation id (cid) flow through every
layer — frontend → HTTP → domain → outbox → dispatcher → outgoing HTTP — appending a
new segment at each communication hop, and turn it into genuinely useful structured
logs. Builds on Specs 1–3.

---

## 1. Goal & motivation

Spec 1 laid partial cid plumbing (a request-span middleware that mints/echoes a flat
`x-correlation-id`, a `correlation_id` column on outbox events, a dispatcher span, an
`http_client` that forwards the header). What it lacks — and what the Haskell
`haskell-service-template` has via `Service.CorrelationId.appendCorrelationId` — is the
**segment-appending mechanism**: the cid is a dotted path that grows a new short segment
at each hop, so the id itself encodes causal lineage and depth. Spec 1 instead copies a
flat uuid onto the event row.

**Goal:** port the segment-appending pattern and make logs first-class, so any single
user action is traceable front-to-back by one growing id, and a user-visible "reference"
on errors lets support `grep` the exact request and everything it spawned.

Non-goals: OpenTelemetry / distributed-tracing exporters, log shipping/aggregation
infrastructure, per-action (vs per-request) cids, and logging the authenticated principal
on the access line (a noted future nicety).

---

## 2. The segment-appending mechanism (the heart of this spec)

A `CorrelationId` is a dotted path of short segments:
`a3f9k2` → `a3f9k2.x7p2qd` → `a3f9k2.x7p2qd.m4n8rb`.

Two primitives in `platform::observability` (mirroring Haskell's `generateCorrelationId`
+ `appendCorrelationId`):
- `new_segment() -> String` — 6 random chars from `[a-z0-9]`. Replaces today's uuid-v4
  `new_correlation_id` (uuids make the path unreadable).
- `append(cid: &str) -> String` — `format!("{cid}.{}", new_segment())`.

**A child segment is minted for each new unit of work.** The rule and hook points:

| Hop | Who appends | Result |
|---|---|---|
| Inbound HTTP request | `correlation_id_middleware` appends to the incoming header value (or to a fresh root segment if absent) | request handled/logged under `root.<seg>` |
| Event published | `OutboxPublisher::publish` stamps the row with `append(current_cid)` | each `outbox_event` row carries its own unique lineage cid |
| Event consumed | dispatcher runs the handler under the event row's cid (no further append — the child was minted at publish) | handler logs share the event's cid |
| Outbound HTTP call | `http_client` forwards the current cid (the callee mints its own child) | downstream extends the lineage |

Worked example (register flow):
```
SPA root:                      r7kq3a
HTTP middleware appends:       r7kq3a.8fp2qd        (request span + access log + echoed on response)
publish user.registered:       r7kq3a.8fp2qd.k1m9xz (row cid)
dispatcher runs account handler under r7kq3a.8fp2qd.k1m9xz
publish account.created:       r7kq3a.8fp2qd.k1m9xz.p4w2nb (row cid)
```
One `grep r7kq3a` reconstructs the whole story; the depth shows how far the cause
propagated.

---

## 3. Backend changes (`platform` + domains)

### 3.1 `platform::observability`
- Replace `new_correlation_id()` (uuid) with `new_segment()` (6 chars `[a-z0-9]`); add
  `append(cid: &str) -> String`. `CorrelationId(String)`, the `FromRequestParts`
  extractor, and `CORRELATION_ID_HEADER` are unchanged.
- `correlation_id_middleware`:
  - `let cid = append(&incoming_header.unwrap_or_else(new_segment));`
  - store `CorrelationId(cid.clone())` in extensions, open `info_span!("request", %cid)`,
    echo `X-Correlation-Id: cid` on the response (unchanged behavior).
  - on completion, emit **one access log**:
    `tracing::info!(method = %m, path = %p, status, latency_ms, "request")` — cid carried
    by the span.
  - **Exclude `/status` and `/metrics`** from the access log and the span (Prometheus
    scrapes `/metrics` every 15s — pure noise).
- `init_tracing(default_level: &str)`: build the filter from
  `EnvFilter::try_from_default_env()` (honors `RUST_LOG`), falling back to `default_level`.
  So `RUST_LOG=debug` works without a rebuild. `main` passes `"info"` as the default.

### 3.2 `platform::events`
- `OutboxPublisher::publish`: stamp the row with `append(&event.correlation_id)`. Producers
  keep passing their *current* cid in `NewEvent.correlation_id`; the publish boundary mints
  the child. (Today it stores the flat cid verbatim — this is the change.)
- Dispatcher: keep `info_span!("event.handle", cid)` built from the row's cid (no extra
  append). Standardize delivered / retry-scheduled / dead-lettered logs on fields
  `delivery_id`, `subscriber`, `event_type`.

### 3.3 Domains (`domain-account`, `domain-auth`)
Add structured logs at the meaningful points, all cid-tagged via the active span:
- auth: register success, login success, login failure (`warn`), logout, scope change.
- account: account created (subscriber), authorization denials (`warn`).
- DLQ: replay.

**Secrets rule (called out in the spec and the reviewer rubric):** never log passwords,
tokens, `Authorization` headers, or full auth request bodies. Log emails, ids, scopes,
event types only.

### 3.4 `http_client`
Contract unchanged (already forwards the cid header); confirm callers pass the current
request cid.

---

## 4. Frontend changes (`web` SPA)

- Add `newSegment()` (6 chars `[a-z0-9]`) in `lib/` (mirrors the backend).
- `fetchClient`: every `apiFetch` mints **one root cid** and sends it as
  `X-Correlation-Id`. The 401→refresh→retry reuses the *same* cid for the retried request
  (one logical request = one root); the internal `/auth/refresh` sub-call gets its own.
- Read the **response** `X-Correlation-Id` (the backend's appended request cid) on every
  response.
- `ApiError` gains `cid?: string`, populated from the response header on non-2xx.
- Error toasts include it: *"Something went wrong — reference: `r7kq3a.8fp2qd`"* — the
  same id that threads the backend trace for that action.
- Optional dev-only `console.debug` of the cid behind `import.meta.env.DEV`.

---

## 5. Testing

- **Backend unit:** `new_segment()` is 6 chars `[a-z0-9]`; `append("abc")` starts with
  `"abc."` and adds exactly one dotted segment.
- **Backend integration (`#[sqlx::test]` / `oneshot`):**
  - middleware appends: a request with `X-Correlation-Id: root` returns a response
    `X-Correlation-Id` that `starts_with("root.")`.
  - publish appends: publishing an event with `correlation_id = "root.a"` yields an
    `outbox_event` row cid `starts_with("root.a.")`.
  - **cid-lineage e2e:** `POST /api/auth/register` with `X-Correlation-Id: root` → the
    `user.registered` row cid begins with `root.`; `dispatch_once` → the resulting
    `account.created` row cid extends that lineage (longer, shares the prefix). This proves
    the chain grows end-to-end.
  - `/metrics` excluded from the request span (light behavioral check; log-text assertions
    are intentionally avoided as brittle).
- **Frontend (Vitest/MSW):** `apiFetch` sends a non-empty `X-Correlation-Id`; on a non-2xx
  carrying a response cid header, the thrown `ApiError.cid` is populated.

---

## 6. Plan decomposition (for writing-plans)

- **Plan A — backend:** `new_segment`/`append`; middleware append + access log +
  `/status`/`/metrics` exclusion; `RUST_LOG`-configurable `init_tracing`; publish
  appends-segment; dispatcher log fields; domain logs + secrets rule; tests.
- **Plan B — frontend:** `newSegment`; fetchClient cid header + same-cid retry +
  response-cid capture; `ApiError.cid`; error-toast reference; tests.

---

## 7. Compatibility notes

- The cid is now a dotted path, not a uuid. Anything that assumed a 36-char uuid cid
  (none in the codebase today) would need updating. Spec 1's `generates_non_empty_cid`
  test (asserts length 36) must be updated to the new segment format.
- The change is backward-compatible at the HTTP boundary: a client that sends no header
  still gets a minted root; a client that sends a flat string still works (it becomes the
  root the backend appends to).
