# workflow-runner Makefile

CARGO_MANIFEST := --manifest-path cli/Cargo.toml

.PHONY: test test-unit test-e2e \
        test-e2e-smoke test-e2e-basic test-e2e-approval test-e2e-agents \
        test-e2e-hooks test-e2e-multi test-e2e-validation test-e2e-resume \
        test-e2e-errors \
        lint build fmt fmt-check ci

# ── all tests ─────────────────────────────────────────────────────────────────

test:
	@echo "Running all tests (unit + E2E)..."
	cargo test $(CARGO_MANIFEST)
	@echo "✓ All tests passed"

# ── unit tests only ───────────────────────────────────────────────────────────

test-unit:
	@echo "Running unit tests..."
	cargo test $(CARGO_MANIFEST) --lib
	@echo "✓ Unit tests passed"

# ── E2E tests (all) ───────────────────────────────────────────────────────────

test-e2e:
	@echo "Running all E2E tests..."
	cargo test $(CARGO_MANIFEST) --test e2e_smoke \
	           --test e2e_basic \
	           --test e2e_approval \
	           --test e2e_agents \
	           --test e2e_hooks \
	           --test e2e_multi_workflow \
	           --test e2e_validation \
	           --test e2e_resume \
	           --test e2e_errors
	@echo "✓ All E2E tests passed"

# ── E2E tests (per scenario) ──────────────────────────────────────────────────

test-e2e-smoke:
	@echo "Running smoke test..."
	cargo test $(CARGO_MANIFEST) --test e2e_smoke
	@echo "✓ Smoke test passed"

test-e2e-basic:
	@echo "Running scenario 1: bug-fix basic lifecycle..."
	cargo test $(CARGO_MANIFEST) --test e2e_basic
	@echo "✓ Scenario 1 passed"

test-e2e-approval:
	@echo "Running scenario 2: approval/reject flow..."
	cargo test $(CARGO_MANIFEST) --test e2e_approval
	@echo "✓ Scenario 2 passed"

test-e2e-agents:
	@echo "Running scenario 3: parallel agents..."
	cargo test $(CARGO_MANIFEST) --test e2e_agents
	@echo "✓ Scenario 3 passed"

test-e2e-hooks:
	@echo "Running scenario 4: hook enforcement..."
	cargo test $(CARGO_MANIFEST) --test e2e_hooks
	@echo "✓ Scenario 4 passed"

test-e2e-multi:
	@echo "Running scenario 5: multiple concurrent workflows..."
	cargo test $(CARGO_MANIFEST) --test e2e_multi_workflow
	@echo "✓ Scenario 5 passed"

test-e2e-validation:
	@echo "Running scenario 6: validate/list/dump-schema..."
	cargo test $(CARGO_MANIFEST) --test e2e_validation
	@echo "✓ Scenario 6 passed"

test-e2e-resume:
	@echo "Running scenario 7: resume..."
	cargo test $(CARGO_MANIFEST) --test e2e_resume
	@echo "✓ Scenario 7 passed"

test-e2e-errors:
	@echo "Running scenario 8: error cases and edge behaviours..."
	cargo test $(CARGO_MANIFEST) --test e2e_errors
	@echo "✓ Scenario 8 passed"

# ── other ─────────────────────────────────────────────────────────────────────

lint:
	@echo "Linting..."
	cargo clippy $(CARGO_MANIFEST) --all-targets --all-features -- -D warnings
	@echo "✓ No issues found"

build:
	@echo "Building..."
	cargo build $(CARGO_MANIFEST) --release
	@echo "✓ Build succeeded"

fmt:
	@echo "Formatting..."
	cargo fmt $(CARGO_MANIFEST) --all
	@echo "✓ Formatting done"

fmt-check:
	@echo "Checking format..."
	cargo fmt $(CARGO_MANIFEST) --all -- --check
	@echo "✓ Format check passed"

# ── CI: fmt-check + lint + all tests ─────────────────────────────────────────

ci: fmt-check lint test
	@echo "✓ CI checks passed"
