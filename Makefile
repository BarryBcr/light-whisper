SHELL := /bin/bash

# Paths
APP_NAME   := Light Whisper
BUNDLE_DIR := src-tauri/target/release/bundle
DEBUG_DIR  := src-tauri/target/debug/bundle
INSTALL_DIR := /Applications

# Python venv for icon generation
VENV       := .venv
PYTHON     := $(VENV)/bin/python3

# ─── Main targets ───

.PHONY: dev build install uninstall icons clean help dev-variant installer-dev test banc

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

dev: icons ## Run in development mode (hot reload)
	cargo tauri dev

build: icons ## Build production release (.app + .dmg)
	cargo tauri build

build-debug: icons ## Build debug version (faster)
	cargo tauri build --debug

install: build ## Build and install to /Applications
	@echo "Installing $(APP_NAME) to $(INSTALL_DIR)..."
	@rm -rf "$(INSTALL_DIR)/$(APP_NAME).app"
	@cp -R "$(BUNDLE_DIR)/macos/$(APP_NAME).app" "$(INSTALL_DIR)/"
	@echo "Installed. Launch from Applications or Spotlight."

uninstall: ## Remove from /Applications and clean user data
	@echo "Removing $(APP_NAME)..."
	@rm -rf "$(INSTALL_DIR)/$(APP_NAME).app"
	@echo "Removed from $(INSTALL_DIR)."
	@echo "User data in ~/lightwhisper/ was kept. Remove manually if needed."

# ─── Icons ───

icons: $(VENV) ## Generate all icons from SVG sources
	@$(PYTHON) scripts/generate-icons.py

$(VENV): scripts/generate-icons.py
	@echo "Setting up Python venv for icon generation..."
	@python3 -m venv $(VENV)
	@$(VENV)/bin/pip install -q cairosvg Pillow
	@touch $(VENV)

# ─── Utilities ───

check: ## Type-check Rust code
	cd src-tauri && cargo check

clippy: ## Run Rust linter
	cd src-tauri && cargo clippy

clean: ## Remove build artifacts
	cd src-tauri && cargo clean
	@rm -rf $(VENV)
	@echo "Cleaned."

# ─── Variante Dev (fork BarryBcr) ───
# Pourquoi ces variables d'environnement :
# - SDKROOT : les Command Line Tools installés (clang 14) ne trouvent pas stdlib.h
#   sans SDK explicite sur macOS 26 ;
# - -U ...MLComputePlan / MLOptimizationHints : le runtime ONNX précompilé référence des
#   classes CoreML absentes du SDK 13.3. macOS 26 les fournit : on les résout à l'exécution ;
# - CMAKE_POLICY_VERSION_MINIMUM : CMake 4 refuse les vieux cmake_minimum_required.
SIGNATURE   := Light Whisper Dev Local
DEV_APP     := Light Whisper Dev
DEV_ENV     := SDKROOT=$$(xcrun --show-sdk-path) CMAKE_POLICY_VERSION_MINIMUM=3.5 \
               RUSTFLAGS='-C link-arg=-Wl,-U,_OBJC_CLASS_$$_MLComputePlan -C link-arg=-Wl,-U,_OBJC_CLASS_$$_MLOptimizationHints'

test: ## Tests unitaires Rust (collage, focus, découpe, journal, config)
	cd src-tauri && $(DEV_ENV) cargo test --features variante-dev

dev-variant: ## Construit « Light Whisper Dev » (.app signé localement). CLI : cargo install tauri-cli --version 2.10.1 --locked
	cd src-tauri && $(DEV_ENV) APPLE_SIGNING_IDENTITY="$(SIGNATURE)" \
		cargo tauri build --features variante-dev --config tauri.dev-variant.conf.json

installer-dev: dev-variant ## Installe « Light Whisper Dev » à côté de l'original (intact)
	@rm -rf "$(INSTALL_DIR)/$(DEV_APP).app"
	@cp -R "$(BUNDLE_DIR)/macos/$(DEV_APP).app" "$(INSTALL_DIR)/"
	@codesign -dv "$(INSTALL_DIR)/$(DEV_APP).app" 2>&1 | grep -E "Identifier|Authority"
	@echo "Installé. Raccourci : Ctrl+Alt+Space. Réglages : ~/lightwhisper-dev/"

banc: ## Banc de vitesse Parakeet (audio FR synthétique, voir bench/)
	cd bench && ./generer-audio.sh && $(DEV_ENV) CARGO_TARGET_DIR=../src-tauri/target cargo run --release -- \
		$$HOME/lightwhisper/models/parakeet-tdt audio/long16k.wav
