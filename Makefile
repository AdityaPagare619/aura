# AURA v4 — Developer Makefile
# Usage: make <target>
# Requires: cargo, rustup, Android NDK (for android targets)
#
# Quick reference:
#   make            — default: check + test
#   make build      — build workspace (stub feature, host)
#   make test       — run all tests (stub feature)
#   make lint       — clippy + fmt check
#   make android    — cross-compile for ARM64 Android
#   make install    — run install.sh in current directory
#   make clean      — cargo clean
#   make audit      — security audit (cargo audit)
#   make deny       — cargo-deny license + advisory check
#   make fmt        — format all code
#   make docs       — generate rustdoc

.PHONY: all build test lint android install clean audit deny fmt docs check release help

# ── Toolchain / Feature flags ─────────────────────────────────────────────────
CARGO         := cargo
STUB_FEATURES := --features stub
ANDROID_TARGET:= aarch64-linux-android
RELEASE_TAG   := v4.0.0-alpha.1

# ── Colors ────────────────────────────────────────────────────────────────────
BOLD  := \033[1m
GREEN := \033[32m
CYAN  := \033[36m
RESET := \033[0m

# ── Default target ────────────────────────────────────────────────────────────
all: check test
	@echo "$(GREEN)$(BOLD)✓ All checks passed$(RESET)"

# ── Compilation ───────────────────────────────────────────────────────────────

## check: Fast type-check without building binaries
check:
	@echo "$(CYAN)→ cargo check (workspace, stub)$(RESET)"
	$(CARGO) check --workspace $(STUB_FEATURES)

## build: Build workspace binaries (host, stub feature — no llama.cpp native)
build:
	@echo "$(CYAN)→ cargo build (workspace, stub)$(RESET)"
	$(CARGO) build --workspace $(STUB_FEATURES)

## build-release: Build workspace in release mode (host, stub)
build-release:
	@echo "$(CYAN)→ cargo build --release (workspace, stub)$(RESET)"
	$(CARGO) build --release --workspace $(STUB_FEATURES)

## android: Cross-compile for aarch64-linux-android (requires NDK)
android:
	@echo "$(CYAN)→ cargo build --release --target $(ANDROID_TARGET)$(RESET)"
	@echo "  NOTE: Requires NDK r26b. Set ANDROID_NDK_HOME or configure .cargo/config.toml"
	$(CARGO) build --release \
		--target $(ANDROID_TARGET) \
		-p aura-daemon \
		-p aura-neocortex

## android-strip: Cross-compile and strip Android binaries
android-strip: android
	@echo "$(CYAN)→ Stripping Android binaries$(RESET)"
	@if [ -z "$$NDK_HOME" ]; then \
		echo "ERROR: NDK_HOME not set"; exit 1; \
	fi
	"$$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" \
		target/$(ANDROID_TARGET)/release/aura-daemon \
		target/$(ANDROID_TARGET)/release/aura-neocortex
	@echo "$(GREEN)✓ Stripped: target/$(ANDROID_TARGET)/release/aura-daemon$(RESET)"
	@echo "$(GREEN)✓ Stripped: target/$(ANDROID_TARGET)/release/aura-neocortex$(RESET)"

# ── Testing ───────────────────────────────────────────────────────────────────

## test: Run all workspace tests (stub feature)
test:
	@echo "$(CYAN)→ cargo test (workspace, stub)$(RESET)"
	RUST_LOG=debug $(CARGO) test --workspace $(STUB_FEATURES)

## test-verbose: Run tests with full output
test-verbose:
	@echo "$(CYAN)→ cargo test --no-fail-fast (workspace, stub)$(RESET)"
	RUST_LOG=debug $(CARGO) test --workspace $(STUB_FEATURES) -- --nocapture --test-threads=1

## test-daemon: Run only aura-daemon tests
test-daemon:
	@echo "$(CYAN)→ cargo test -p aura-daemon (stub)$(RESET)"
	RUST_LOG=debug $(CARGO) test -p aura-daemon $(STUB_FEATURES)

## test-neocortex: Run only aura-neocortex tests
test-neocortex:
	@echo "$(CYAN)→ cargo test -p aura-neocortex (stub)$(RESET)"
	RUST_LOG=debug $(CARGO) test -p aura-neocortex $(STUB_FEATURES)

# ── Linting ───────────────────────────────────────────────────────────────────

## lint: Run clippy + format check
lint: clippy fmt-check

## clippy: Clippy with warnings as errors
clippy:
	@echo "$(CYAN)→ cargo clippy (workspace, stub, -D warnings)$(RESET)"
	$(CARGO) clippy --workspace $(STUB_FEATURES) -- -D warnings

## fmt: Format all code
fmt:
	@echo "$(CYAN)→ cargo fmt$(RESET)"
	$(CARGO) fmt --all

## fmt-check: Check formatting without modifying files
fmt-check:
	@echo "$(CYAN)→ cargo fmt --check$(RESET)"
	$(CARGO) fmt --all --check

# ── Security ──────────────────────────────────────────────────────────────────

## audit: Security audit for known CVEs in dependencies
audit:
	@echo "$(CYAN)→ cargo audit$(RESET)"
	$(CARGO) audit

## deny: License and advisory check via cargo-deny
deny:
	@echo "$(CYAN)→ cargo deny check$(RESET)"
	$(CARGO) deny check

## security: Run all security checks
security: audit deny

# ── Documentation ─────────────────────────────────────────────────────────────

## docs: Generate rustdoc (opens in browser)
docs:
	@echo "$(CYAN)→ cargo doc (workspace, stub)$(RESET)"
	$(CARGO) doc --workspace $(STUB_FEATURES) --no-deps --open

## docs-no-open: Generate rustdoc without opening browser
docs-no-open:
	$(CARGO) doc --workspace $(STUB_FEATURES) --no-deps

# ── Installation ──────────────────────────────────────────────────────────────

## install: Run the Termux installer (for on-device use)
install:
	@echo "$(CYAN)→ Running install.sh$(RESET)"
	bash install.sh

## install-skip-build: Install using pre-built binaries from GitHub Releases
install-skip-build:
	@echo "$(CYAN)→ Running install.sh --skip-build$(RESET)"
	bash install.sh --skip-build

## install-dry-run: Preview what the installer would do
install-dry-run:
	@echo "$(CYAN)→ Running install.sh --dry-run$(RESET)"
	bash install.sh --dry-run

# ── Release workflow ──────────────────────────────────────────────────────────

## tag: Create a version tag and push to trigger the release pipeline
## Usage: make tag VERSION=v4.0.0-alpha.2
tag:
	@if [ -z "$(VERSION)" ]; then \
		echo "ERROR: VERSION not set. Usage: make tag VERSION=v4.0.0-alpha.2"; \
		exit 1; \
	fi
	@echo "$(CYAN)→ Creating tag $(VERSION)$(RESET)"
	git tag -a "$(VERSION)" -m "Release $(VERSION)"
	git push origin "$(VERSION)"
	@echo "$(GREEN)✓ Tag $(VERSION) pushed — GitHub Actions release pipeline triggered$(RESET)"

## release-local: Build and package release artifacts locally (Android)
release-local: android-strip
	@echo "$(CYAN)→ Packaging release artifacts for $(RELEASE_TAG)$(RESET)"
	@mkdir -p dist
	@for BIN in aura-daemon aura-neocortex; do \
		cp "target/$(ANDROID_TARGET)/release/$$BIN" \
			"dist/$${BIN}-$(RELEASE_TAG)-$(ANDROID_TARGET)"; \
		sha256sum "dist/$${BIN}-$(RELEASE_TAG)-$(ANDROID_TARGET)" \
			> "dist/$${BIN}-$(RELEASE_TAG)-$(ANDROID_TARGET).sha256"; \
		echo "$(GREEN)✓ $${BIN}-$(RELEASE_TAG)-$(ANDROID_TARGET)$(RESET)"; \
	done
	@echo "$(GREEN)✓ Artifacts in dist/$(RESET)"

# ── Cleanup ───────────────────────────────────────────────────────────────────

## clean: Remove build artifacts
clean:
	@echo "$(CYAN)→ cargo clean$(RESET)"
	$(CARGO) clean
	@rm -rf dist/
	@echo "$(GREEN)✓ Cleaned$(RESET)"

## clean-target: Remove only target/ (keep dist/)
clean-target:
	$(CARGO) clean

# ── Help ──────────────────────────────────────────────────────────────────────

## help: Show this help
help:
	@echo "$(BOLD)AURA v4 — Make Targets$(RESET)"
	@echo ""
	@grep -E '^## ' Makefile | sed 's/## /  /' | column -t -s ':'
	@echo ""
	@echo "$(BOLD)Common workflows:$(RESET)"
	@echo "  $(CYAN)make$(RESET)                  — check + test (default)"
	@echo "  $(CYAN)make lint$(RESET)              — clippy + fmt check"
	@echo "  $(CYAN)make android$(RESET)           — cross-compile for ARM64 Android"
	@echo "  $(CYAN)make tag VERSION=v4.x$(RESET)  — create release tag and push"
	@echo "  $(CYAN)make install-skip-build$(RESET) — install pre-built binary on Termux"
