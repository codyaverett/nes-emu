# Self-documenting Makefile for nes-emu. Run `make` or `make help` for the
# target list; every target with a `##` comment shows up there.
#
# Variables you can override on the command line:
#   ROM=roms/mario.nes        ROM used by run/run-debug/screenshots
#   ARGS="--no-audio"         extra flags passed to the emulator
#   OUT=/tmp/shots            output directory for screenshots

ROM  ?= roms/mario.nes
ARGS ?=
OUT  ?= target/screenshots

.DEFAULT_GOAL := help

##@ General

help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "; printf "\nnes-emu targets\n\n"} \
	     /^[a-zA-Z_-]+:.*?## / { printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2 } \
	     /^##@ / { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)
	@printf "\nVariables: ROM=%s ARGS=%s OUT=%s\n\n" "$(ROM)" "$(ARGS)" "$(OUT)"

setup: ## Install the toolchain bits (SDL2, wasm target, wasm-pack)
	brew install sdl2
	rustup target add wasm32-unknown-unknown
	cargo install wasm-pack

clean: ## Remove cargo and wasm build output
	cargo clean
	rm -rf web/pkg web/pkg-node

##@ Native (SDL frontend, default `sdl` feature)

build: ## Debug build of the workspace
	cargo build --workspace

release: ## Optimised build of the workspace
	cargo build --workspace --release

run: ## Run the emulator on $(ROM) with $(ARGS)
	cargo run --release -- $(ROM) $(ARGS)

run-debug: ## Run $(ROM) with RUST_LOG=debug (controller input logging)
	RUST_LOG=debug cargo run -- $(ROM) $(ARGS)

##@ Checks (the definition of done, mirroring .github/workflows/ci.yml)

check: build test clippy fmt-check ## Everything CI runs for the native target

test: ## Run the workspace test suite
	cargo test --workspace

clippy: ## Lint with warnings denied
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format the workspace
	cargo fmt --all

fmt-check: ## Fail if anything is unformatted
	cargo fmt --all --check

core-test: ## Test the library without the sdl feature (wasm-compatible core)
	cargo test --no-default-features

screenshots: ## Capture the UI pages of $(ROM) as PPM into $(OUT)
	scripts/ui_screenshots.sh $(ROM) $(OUT)

##@ Web (WebAssembly frontend in web/)

wasm: ## Build the wasm crate for wasm32-unknown-unknown
	cargo build -p nes-emu-web --release --target wasm32-unknown-unknown

web-build: ## wasm-pack build of the browser bundle (web/pkg)
	cd web && npm run build

web-test: ## Web unit tests plus the Node smoke test
	cd web && npm test

web-size: ## Check the wasm size budget (500 KB, needs web-test first)
	cd web && npm run check-size -- pkg-node/nes_emu_web_bg.wasm

web-serve: ## Serve web/ on http://127.0.0.1:8080/ (run web-build first)
	cd web && npm run serve

web-check: web-test web-size ## Everything CI runs for the web target

.PHONY: help setup clean build release run run-debug check test clippy fmt \
        fmt-check core-test screenshots wasm web-build web-test web-size \
        web-serve web-check
