# Build, test and install syswatch.
#
# Every target is safe to run repeatedly. `make` alone builds a debug binary,
# `make release` the optimized one. See `make help` for the full list.

BINARY      := syswatch
CARGO       ?= cargo
CARGO_FLAGS ?=
DESTDIR     ?=
PREFIX      ?= $${HOME}/.local
BINDIR      ?= $(PREFIX)/bin
CONFIG_DIR  ?= $(DESTDIR)$${XDG_CONFIG_HOME:-$${HOME}/.config}/syswatch
TARGET_DIR  ?= target
RELEASE_BIN := $(TARGET_DIR)/release/$(BINARY)
DEBUG_BIN   := $(TARGET_DIR)/debug/$(BINARY)
SOURCE_DATE ?= $(shell date -u +%Y-%m-%dT%H:%M:%SZ)

.DEFAULT_GOAL := help
.PHONY: help build release test check fmt fmt-check clippy lint run clean install uninstall \
        config verify smoke all-checks

help: ## Show this help
	@echo "syswatch - available targets:"
	@grep -hE '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo ""

build: ## Build the debug binary
	$(CARGO) build $(CARGO_FLAGS)

release: ## Build the optimized binary
	$(CARGO) build --release $(CARGO_FLAGS)

run: build ## Build and run the debug binary
	./$(DEBUG_BIN)

test: ## Run the test suite
	$(CARGO) test $(CARGO_FLAGS)

check: ## Type-check every target without running anything
	$(CARGO) check --all-targets $(CARGO_FLAGS)

fmt: ## Format the source
	$(CARGO) fmt --all

fmt-check: ## Fail if the source is not formatted
	$(CARGO) fmt --all -- --check

clippy: ## Lint with clippy, denying warnings
	$(CARGO) clippy --all-targets -- -D warnings

lint: fmt-check clippy ## Formatting and lints only

all-checks: fmt-check check clippy test ## Every CI check, locally

config: ## Print the configuration that would be used
	./$(RELEASE_BIN) --dump-config

verify: release ## Build, then check that the binary answers --version
	./$(RELEASE_BIN) --version >/dev/null
	./$(RELEASE_BIN) --help >/dev/null
	@echo "binary OK: $(RELEASE_BIN)"

smoke: release ## Drive the TUI in a pseudo terminal and print the screen
	python3 scripts/smoke.py

install: release ## Install to ~/.local/bin and seed the default configuration
	./install.sh --prefix "$(PREFIX)"

uninstall: ## Remove the installed binary and optionally the configuration
	./uninstall.sh

clean: ## Remove build artifacts
	$(CARGO) clean
