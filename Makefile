# Lion-Heart — developer convenience wrapper around cargo.
#
# Quick start:
#   make install   # build release + install the `lh` launcher on your PATH
#   make run       # launch the standalone GUI (cargo run, release)
#   make test      # run the workspace tests
#   make check     # fmt + clippy + test — the pre-commit gate
#
# `make install` copies the release binary to $(BINDIR)/$(BIN). Override the
# destination or the launcher name:
#   make install BINDIR=/usr/local/bin      # system-wide (may need sudo)
#   make install BIN=lion-heart             # keep the full name

CARGO       ?= cargo
APP          = lion-heart
BIN         ?= lh
BINDIR      ?= $(HOME)/.cargo/bin
RELEASE_BIN  = target/release/$(APP)

.DEFAULT_GOAL := help

.PHONY: help build release run test fmt fmt-check lint clippy check bench bundle install uninstall clean

help: ## Show this help
	@printf 'Lion-Heart — make targets\n\n'
	@grep -E '^[a-zA-Z_-]+:.*## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*## "} {printf "  \033[36m%-11s\033[0m %s\n", $$1, $$2}'
	@printf '\ninstall target: %s\n' "$(BINDIR)/$(BIN)"

build: ## Debug build of the whole workspace
	$(CARGO) build

release: ## Optimized release build
	$(CARGO) build --release

run: ## Launch the standalone GUI (release)
	$(CARGO) run -p $(APP) --release

test: ## Run all workspace tests (offline, no audio device)
	$(CARGO) test

fmt: ## Format the workspace
	$(CARGO) fmt

fmt-check: ## Check formatting without writing (CI gate)
	$(CARGO) fmt --check

clippy: ## Clippy with warnings denied (CI gate)
	$(CARGO) clippy --all-targets -- -D warnings

lint: clippy ## Alias for clippy

check: fmt-check clippy test ## Pre-commit gate: fmt-check + clippy + test

bench: ## Per-block DSP cost (criterion)
	$(CARGO) bench -p lh-dsp --bench effects

bundle: ## Build the CLAP/VST3 plugin into target/bundled
	$(CARGO) xtask bundle lion-heart-plugin --release

install: release ## Build release + install the `lh` launcher into $(BINDIR)
	@mkdir -p "$(BINDIR)"
	install -m 0755 "$(RELEASE_BIN)" "$(BINDIR)/$(BIN)"
	@printf 'installed: %s\n' "$(BINDIR)/$(BIN)"
	@echo "$$PATH" | tr ':' '\n' | grep -qx "$(BINDIR)" \
		&& printf 'run it with:  %s\n' "$(BIN)" \
		|| printf 'note: add %s to your PATH to run `%s` directly\n' "$(BINDIR)" "$(BIN)"

uninstall: ## Remove the installed `lh` launcher
	rm -f "$(BINDIR)/$(BIN)"
	@printf 'removed: %s\n' "$(BINDIR)/$(BIN)"

clean: ## cargo clean (removes ./target)
	$(CARGO) clean
