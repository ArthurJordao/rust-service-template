DATABASE_URL ?= postgres://postgres:postgres@localhost:5432/app

.PHONY: up down run test migrate fmt lint new-domain web-install web-dev web-build web-test web-lint gen-keys

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

web-install:
	npm --prefix web ci

web-dev:
	npm --prefix web run dev

web-build:
	npm --prefix web run build

web-test:
	npm --prefix web test

web-lint:
	npm --prefix web run lint

gen-keys:
	mkdir -p secrets
	openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out secrets/jwt_private.pem
	openssl rsa -pubout -in secrets/jwt_private.pem -out secrets/jwt_public.pem
	@echo "Wrote secrets/jwt_private.pem and secrets/jwt_public.pem (gitignored)."

.PHONY: gen-api
gen-api:
	cargo run --quiet --locked -p app --bin openapi-gen > web/openapi.json
	npm --prefix web exec -- openapi-typescript web/openapi.json -o web/src/api/schema.d.ts
