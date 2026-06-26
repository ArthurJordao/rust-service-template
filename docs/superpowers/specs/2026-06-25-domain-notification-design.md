# domain-notification — Design (Spec 4)

**Date:** 2026-06-25
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** A new `domain-notification` crate that subscribes to
`account.created`, renders a `handlebars` template, dispatches through a pluggable
`Notifier` port (dev impl logs it), and records a `sent_notification` row. Builds on
Specs 1–3 (and pairs with the correlation-id/logging spec).

---

## 1. Goal & principles

Add the first event-driven side-effecting domain, mirroring the Haskell
`notification-service` (Mustache templates, `SentNotification` record, Email-only stub
dispatch) but adapted to this monolith's in-process outbox. It consumes `account.created`
— which `domain-account` already emits but **nothing currently consumes** — completing
the event chain `user.registered → account.created → notification` and demonstrating a
multi-domain reaction through the outbox.

Invariants preserved from Spec 1:
- **Hexagonal**: pure rules in `domain.rs`; adapters under `ports/`.
- **Ports are traits; DI via `Arc<dyn Port>`** (`Notifier`, `SentNotificationRepository`).
- **One crate per domain; cross-domain via events only** — `domain-notification` does NOT
  depend on `domain-account`; it deserializes the `account.created` payload by shape into
  a local type.
- **sqlx runtime query API**; **idempotent handlers** (at-least-once delivery).

---

## 2. Decisions (resolved during brainstorming)

1. **Trigger: subscribe to domain events directly.** `domain-notification` subscribes to
   `account.created` and sends a welcome notification — self-contained, no new producer,
   and it consumes a previously-unconsumed event. (A generic `notification.requested`
   event is a documented future extension.)
2. **Templating: `handlebars` crate, templates embedded** via `include_str!` (single
   binary, no runtime file dependency). Mirrors the Haskell Mustache heritage.
3. **Channels: `Email` only, behind a `Notifier` port**; the dev `LogNotifier` logs the
   rendered notification (cid-tagged). Real providers (SMTP/SES/Resend) are future
   adapters behind the same trait.
4. **State + idempotency:** a `sent_notification` table with a **unique `source_event_id`**;
   the handler is idempotent (redelivery is a no-op).

---

## 3. Crate layout

```
crates/domain-notification/src/
  domain.rs                  # pure: build template name + variables + recipient from the event
  models.rs                  # SentNotification, NotificationChannel
  templates/welcome.txt.hbs  # embedded via include_str!
  ports/
    repository.rs            # SentNotificationRepository trait + Postgres adapter
    notifier.rs              # Notifier trait + LogNotifier (dev)
    templates.rs             # Templates (handlebars registry) + render(name, vars)
    events.rs                # NotificationSubscriber: on account.created -> render -> notify -> record
    http.rs                  # GET /notifications (admin) + NotificationState + router()
  lib.rs                     # pub use router, NotificationState, NotificationSubscriber
```
Dependency graph stays acyclic: `domain-notification → platform`; `app → domain-notification`.
New workspace dep: `handlebars` (one mature crate).

---

## 4. Flow

```
domain-account publishes account.created { account_id, auth_user_id, email }
        │   (outbox row; cid lineage continues from the originating request)
        ▼   (dispatcher, async)
NotificationSubscriber  (name "notification.on-account-created", event_type "account.created")
   1. deserialize local AccountCreated { account_id, auth_user_id, email }
   2. idempotency: find_by_event_id(event.event_id) — if present, skip (no-op)
   3. render "welcome" template with { email, account_id }
   4. notifier.send(Email(email), subject, body)        [LogNotifier logs it, cid-tagged]
   5. record sent_notification(source_event_id = event.event_id, template, channel,
                               recipient, body, created_by_cid = event.correlation_id)
```
`Routes` gains `account.created → notification.on-account-created`. Combined with the
existing `user.registered → account.on-user-registered`, this is the chain
`user.registered → account.created → notification`.

**Failure → DLQ:** the handler returns `Err` on unknown template / render error → the
outbox retries with exponential backoff → dead-letters (visible in the Spec-3 DLQ admin
page). A recipient suffix `@fail.test` is a deterministic failure hook for tests.

---

## 5. Components

### 5.1 Migration `0006_notification.sql`
```sql
create table sent_notification (
    id              bigserial primary key,
    source_event_id bigint      not null unique,
    template        text        not null,
    channel         text        not null,
    recipient       text        not null,
    body            text        not null,
    created_at      timestamptz not null default now(),
    created_by_cid  text        not null
);
```

### 5.2 `models.rs`
- `SentNotification { id, source_event_id, template, channel, recipient, body, created_at, created_by_cid }` — `Debug, Clone, Serialize, sqlx::FromRow`.
- `NotificationChannel` enum — `Email(String)` for now; mapped to a `channel` text
  (`"email"`) + `recipient` for storage.

### 5.3 `ports/templates.rs`
`Templates` wraps `handlebars::Handlebars<'static>`, built once at startup, registering
the embedded `welcome.txt.hbs`. `render(&self, name: &str, vars: &serde_json::Value) ->
anyhow::Result<String>`; unknown template or strict-mode missing variable → `Err`. Held
as `Arc<Templates>`. (handlebars strict mode on, so missing vars fail loudly → DLQ.)

### 5.4 `ports/notifier.rs`
```rust
#[async_trait] pub trait Notifier: Send + Sync {
    async fn send(&self, channel: &NotificationChannel, subject: &str, body: &str) -> anyhow::Result<()>;
}
pub struct LogNotifier;   // dev: tracing::info!(recipient, "notification dispatched")
```
Real providers implement the same trait later. `Arc<dyn Notifier>`.

### 5.5 `ports/repository.rs`
`SentNotificationRepository` — `find_by_event_id(i64) -> anyhow::Result<Option<SentNotification>>`,
`record(NewSentNotification) -> anyhow::Result<()>`, `list() -> anyhow::Result<Vec<SentNotification>>`.
`PostgresSentNotificationRepository` (sqlx runtime API).

### 5.6 `ports/events.rs`
`NotificationSubscriber { repo: Arc<dyn SentNotificationRepository>, notifier: Arc<dyn Notifier>, templates: Arc<Templates> }` impl `Subscriber` per §4. Local
`#[derive(Deserialize)] struct AccountCreated { account_id: i64, auth_user_id: i64, email: String }`.

### 5.7 `ports/http.rs`
`NotificationState { repo, jwt: Arc<JwtVerifier>, revocation: Arc<dyn RevocationChecker>, metrics }`
+ `FromRef` impls; `GET /notifications` (admin-gated) → `Json<Vec<SentNotification>>`;
`router(NotificationState)`.

### 5.8 App wiring (`app/state.rs`, `build_router`)
- `Routes::new()…add("account.created", "notification.on-account-created")`.
- Register `NotificationSubscriber` in the `SubscriberRegistry`.
- Build `NotificationState`; merge `domain_notification::router` into the `/api` router.

---

## 6. Testing

- **Unit (no DB):** `render("welcome", {email, account_id})` body contains the email;
  unknown template → `Err`; the local `AccountCreated` deserializes from the
  `account.created` JSON shape.
- **Integration (`#[sqlx::test]`):**
  - subscriber handles `account.created` → exactly one `sent_notification` row (recipient,
    `source_event_id`); `LogNotifier` used.
  - idempotent: handling the same event twice → one row.
  - failure → `Err` for a `@fail.test` recipient (proves retry/dead-letter via
    `dispatch_once` + delivery status).
  - `GET /notifications` without a token → 401.
- **e2e (`app`):** `register` → `dispatch_once` (account created + `account.created`
  emitted) → `dispatch_once` (notification consumes it) → assert a `sent_notification`
  exists — exercises the full `user.registered → account.created → notification` chain.

---

## 7. Plan decomposition (for writing-plans)

Likely a single plan (~7–8 tasks): migration + models; templates + notifier; repository;
subscriber (+ idempotency); admin `GET /notifications`; app wiring; e2e. `writing-plans`
may split into 4a (crate + templates + notifier + repo) and 4b (subscriber + http +
wiring + e2e) if warranted.

---

## 8. Out of scope / future

- Real email providers (SMTP/SES/Resend) as `Notifier` adapters.
- SPA Notifications admin page (this spec ships the backend `GET /notifications`; the page
  is a small frontend follow-up mirroring Spec 3's `UsersPage`).
- A generic `notification.requested` event for ad-hoc/templated sends from any domain.
- User notification preferences / opt-out; additional channels (SMS/push — enum is
  extensible); localization.
