#!/usr/bin/env bash
set -euo pipefail

name="${1:?usage: new-domain.sh <name>}"
crate="domain-${name}"
dir="crates/${crate}"

if [ -d "$dir" ]; then
  echo "error: $dir already exists" >&2
  exit 1
fi

mkdir -p "$dir/src/ports" "$dir/tests"

cat > "$dir/Cargo.toml" <<EOF
[package]
name = "${crate}"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true
anyhow.workspace = true
chrono.workspace = true
http.workspace = true
EOF

cat > "$dir/src/lib.rs" <<EOF
//! ${name} domain.
pub mod domain;
pub mod models;
pub mod ports;
EOF

cat > "$dir/src/domain.rs" <<EOF
//! Pure business rules for the ${name} domain.
EOF

cat > "$dir/src/models.rs" <<EOF
//! ${name} data models.
EOF

cat > "$dir/src/ports/mod.rs" <<EOF
pub mod repository;
EOF

cat > "$dir/src/ports/repository.rs" <<EOF
//! ${name} repository port + Postgres adapter.
EOF

echo "Scaffolded ${dir}. Add it to the workspace members in Cargo.toml."
