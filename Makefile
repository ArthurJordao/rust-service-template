DATABASE_URL ?= postgres://postgres:postgres@localhost:5432/app

.PHONY: up down run test migrate fmt lint new-domain

up:
	docker compose up -d postgres prometheus grafana

down:
	docker compose down

run:
	cargo run -p app

test:
	DATABASE_URL=$(DATABASE_URL) cargo test

migrate:
	DATABASE_URL=$(DATABASE_URL) sqlx migrate run

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

new-domain:
	./scripts/new-domain.sh $(name)
