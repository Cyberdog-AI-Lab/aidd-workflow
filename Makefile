# workflow-runner フェーズ2実験用 Makefile

.PHONY: test lint build fmt fmt-check

test:
	@echo "Running tests..."
	cargo test

lint:
	@echo "Linting..."
	cargo clippy --all-targets --all-features -- -D warnings
	@echo "✓ No issues found"

build:
	@echo "Building..."
	cargo build --release
	@echo "✓ Build succeeded"

fmt:
	@echo "Formatting..."
	cargo fmt --all
	@echo "✓ Formatting done"

fmt-check:
	@echo "Formatting..."
	cargo fmt --all -- --check
	@echo "✓ Format check passed"
