# utoipa Plan B: Frontend typed client (openapi-typescript) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate TypeScript from the backend's OpenAPI doc and type the SPA's existing fetch client + api modules against it, so a wrong path / wrong params / mismatched body is a `tsc` error — without replacing the custom `fetchClient` (401 refresh, cid, `ApiError`).

**Architecture:** A `make gen-api` target runs the `openapi-gen` bin → `web/openapi.json` → `openapi-typescript` → committed `web/src/api/schema.d.ts`. `api/types.ts` becomes thin aliases of `components["schemas"][…]` (authoritative DTO shapes), and `apiFetch` is constrained to the generated `paths` so calling a path/method/body that the API doesn't have fails compilation.

**Tech Stack:** openapi-typescript (devDep), TypeScript, the existing Vite/Vitest SPA.

## Global Constraints

- **Depends on Plan A** (the `openapi-gen` bin + `/api/openapi.json`).
- **openapi-typescript (types only) — NOT openapi-fetch.** Keep the custom `fetchClient` (single-flight 401→refresh, `X-Correlation-Id`, `ApiError`) unchanged.
- `web/src/api/schema.d.ts` is **committed**; `web/openapi.json` is a build artifact (gitignored).
- TS `strict`. `npm run build` (tsc) + `npm run lint` + `npm test -- run` must pass. Run npm from `web/`.
- Node 26 / npm 11. Commit prefix `chore(web):` / `feat(web):`.

---

### Task 1: gen tooling + commit `schema.d.ts`

**Files:** Modify `Makefile`, `web/package.json` (devDep), `web/.gitignore`; create `web/src/api/schema.d.ts` (generated, committed).

**Interfaces:** Produces: `make gen-api`; the committed `schema.d.ts` with `paths` + `components` for the whole API.

- [ ] **Step 1: Add `openapi-typescript` devDep**

In `web/`: `npm install -D openapi-typescript`

- [ ] **Step 2: Add the `gen-api` Makefile target**

Append to the root `Makefile`:
```makefile
.PHONY: gen-api
gen-api:
	cargo run --quiet -p app --bin openapi-gen > web/openapi.json
	npm --prefix web exec -- openapi-typescript web/openapi.json -o web/src/api/schema.d.ts
```

- [ ] **Step 3: gitignore the intermediate spec**

Append to `web/.gitignore` (or root `.gitignore` under the web section):
```
openapi.json
```

- [ ] **Step 4: Generate**

From repo root: `make gen-api`
Expected: writes `web/src/api/schema.d.ts` (contains `export interface paths` and `export interface components`). Inspect it: confirm path keys (e.g. `"/auth/login"`, `"/accounts/me"`, `"/admin/dlq"`) and schema names (`AuthTokens`, `Account`, `DeadLetter`). **Note the exact path-key form** — whether keys include `/api` or are domain-relative (driven by the `servers` entry); Task 2 adapts to whichever the generator produced.

- [ ] **Step 5: Verify it type-checks**

In `web/`: `npm run build`
Expected: PASS (the new `schema.d.ts` is valid TS; nothing imports it yet).

- [ ] **Step 6: Commit**
```bash
git add Makefile web/package.json web/package-lock.json web/.gitignore web/src/api/schema.d.ts
git commit -m "chore(web): generate committed OpenAPI types (openapi-typescript) + make gen-api"
```

---

### Task 2: Retype `api/types.ts` + path-constrain `apiFetch`

**Files:** Modify `web/src/api/types.ts`, `web/src/lib/fetchClient.ts`, and (only if needed) the api modules `web/src/api/{auth,accounts,users,dlq}.ts`.

**Interfaces:**
- Consumes: `schema.d.ts` (`paths`, `components`).
- Produces: DTO types are schema-derived; `apiFetch` path argument constrained to the API's real paths.

- [ ] **Step 1: Make `api/types.ts` schema-derived**

Replace the hand-written interfaces in `web/src/api/types.ts` with aliases from the generated schema (keeps every existing import working, now authoritative):
```ts
import type { components } from "@/api/schema";

export type AuthTokens = components["schemas"]["AuthTokens"];
export type Account = components["schemas"]["Account"];
export type UserWithScopes = components["schemas"]["UserWithScopes"];
export type ScopeInfo = components["schemas"]["ScopeRow"];   // backend type is ScopeRow
export type DeadLetter = components["schemas"]["DeadLetter"];
```
> If a generated schema field is optional/nullable where the hand-written type was required (or vice-versa), fix the call sites the compiler flags — that mismatch is exactly the contract drift this spec exists to surface. (e.g. `last_error` is `string | null`.)

- [ ] **Step 2: Run build to surface any drift**

In `web/`: `npm run build`
Expected: either PASS, or `tsc` errors at real shape mismatches between the old hand-written types and the contract — fix each at the call site (these are genuine corrections). Re-run until green.

- [ ] **Step 3: Path-constrain `apiFetch`**

In `web/src/lib/fetchClient.ts`, constrain the path argument to the API's paths so a typo or non-existent route fails compilation. Add at the top:
```ts
import type { paths } from "@/api/schema";

export type ApiPath = keyof paths;
```
Change the `apiFetch` signature so `path` is `ApiPath` (the generated path keys). Keep the body/return generic `<T>` for now (full per-path body/response inference via `paths[P][M]` is possible but version-sensitive; the path constraint already catches wrong/removed routes, and Step 1 makes bodies/responses schema-derived through `api/types.ts`):
```ts
export async function apiFetch<T>(path: ApiPath, opts: Opts = {}): Promise<T> { /* body unchanged */ }
```
> If the generated path keys are domain-relative (no `/api`) but the api modules call `"/auth/login"`, they already match `ApiPath`; if the keys include `/api`, either set the api-module paths accordingly or keep `apiFetch` accepting `\`${ApiPath}\` | string`-free by aligning to the generated form noted in Task 1 Step 4. Pick the form that makes the api modules compile against `ApiPath` with no `as` casts.

- [ ] **Step 4: Verify api modules compile against the constrained path**

In `web/`: `npm run build`
Expected: the `auth.ts`/`accounts.ts`/`users.ts`/`dlq.ts` calls type-check against `ApiPath`. Fix any path string that doesn't match a real route (a genuine bug surfaced).

- [ ] **Step 5: Full gate**

In `web/`: `npm run build && npm run lint && npm test -- run`
Expected: all pass (the 8 Vitest tests still green — MSW handlers are unaffected by types).

- [ ] **Step 6: Commit**
```bash
git add web/src
git commit -m "feat(web): type api modules + apiFetch against generated OpenAPI schema"
```

---

### Task 3: README + drift note

**Files:** Modify `README.md`.

- [ ] **Step 1: Document the workflow**

Add under the Frontend section of `README.md`:
```markdown
### Typed API client

The SPA's request/response types are generated from the backend's OpenAPI doc:

    make gen-api        # openapi-gen bin -> web/openapi.json -> web/src/api/schema.d.ts (committed)

Run it after changing any handler/DTO. Swagger UI is served at `/swagger-ui`, the raw
spec at `/api/openapi.json`. A wrong path or mismatched body is a `tsc` error
(`npm run build`).
```
(Optionally note a future CI drift-check: run `make gen-api` and fail if `git diff --exit-code web/src/api/schema.d.ts` is dirty — out of scope until CI exists.)

- [ ] **Step 2: Commit**
```bash
git add README.md
git commit -m "docs(web): document the typed-client (gen-api) workflow"
```

---

## Self-Review

**Spec coverage (design §3):** gen tooling + committed schema.d.ts + gen-api target ✓ (T1); api/types.ts schema-derived ✓ (T2 S1); apiFetch path-constrained ✓ (T2 S3); build/lint/test green ✓ (T2 S5); workflow docs + drift note ✓ (T3). The path-key `/api`-prefix ambiguity (design §6) is resolved at T1 S4 by inspecting the generated output and adapting in T2 S3.

**Placeholder scan:** no TBDs. The two adaptive notes (optional/nullable field fixes; path-key form) are deliberate contract-drift handling with concrete instructions — they are the point of the spec, not hand-waving.

**Type consistency:** `ScopeInfo` aliases the backend `ScopeRow` schema (names reconciled). `ApiPath = keyof paths` used in `apiFetch` (T2 S3) and satisfied by the api modules (T2 S4). DTO aliases (T2 S1) feed the existing api modules unchanged. Full per-path body/response inference is intentionally deferred (path-constraint + schema-derived DTOs deliver the practical guarantee without version-fragile generics).

**Dependency:** Task 1 requires Plan A's `openapi-gen` bin and served `/api/openapi.json`.
