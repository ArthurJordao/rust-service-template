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
- `crates/domain-*` — one crate per domain (pure rules + ports)
- `crates/app` — composition root: wires domains, runs server + outbox dispatcher

## Add a domain

    make new-domain name=billing
