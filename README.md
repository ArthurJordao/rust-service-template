# rust-service-template

Idiomatic-Rust service template: a monolith of internal domains with a
transactional outbox, correlation-id tracing, JWT auth, and Prometheus metrics.

## Quick start

    cp .env.example .env        # edit JWT key etc.
    make up                     # start Postgres + Prometheus + Grafana
    make migrate                # apply migrations
    make run                    # start the app on :8080

## Test

    make up
    make test                   # needs DATABASE_URL pointing at Postgres

## Architecture

See `docs/superpowers/specs/2026-06-24-rust-service-template-design.md`.

- `crates/platform` — cross-cutting: config, db, events (outbox), auth, metrics, http client, observability
- `crates/domain-auth` — register/login/refresh/logout, RS256 JWTs, **Postgres-backed**
  token revocation (no Redis), admin scope management
- `crates/domain-*` — one crate per domain (pure rules + ports)
- `crates/app` — composition root: wires domains, runs server + outbox dispatcher

## Frontend (web SPA)

    make web-install            # install deps (web/)
    make web-dev                # Vite dev server on :5173 (proxies /api -> :8080)
    make web-build              # build to web/dist; `make run` then serves it at :8080

### Typed API client

The SPA's request/response types are generated from the backend's OpenAPI doc:

    make gen-api        # openapi-gen bin -> web/openapi.json -> web/src/api/schema.d.ts (committed)

Run it after changing any handler or DTO. Swagger UI is served at `/swagger-ui`,
the raw spec at `/api/openapi.json`. A wrong path or mismatched body is a `tsc`
error (`npm run build`): `apiFetch`'s path is constrained to the API's real
routes, and `web/src/api/types.ts` aliases the generated schemas.

> CI drift-check (once CI exists): run `make gen-api` and fail if
> `git diff --exit-code web/src/api/schema.d.ts` is dirty.

## Add a domain

    make new-domain name=billing
