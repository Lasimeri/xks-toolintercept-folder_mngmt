# xks-toolintercept-folder_mngmt
#
#   make                           build xtf (release)
#   make check                     unit tests + clippy (no model needed)
#   make install [PREFIX=~/.local] copy xtf to $(PREFIX)/bin
#   make install-claude            the Claude Code plugin (hooks + MCP) for every
#                                  project, and the jevbase user service
#   make calibrate REPOS=DIR       run the committed calibration set against
#                                  the decision server (TYPESAFE_BASE_URL,
#                                  default http://127.0.0.1:8097); DIR holds
#                                  checkouts of the repositories it names
#
# Builds use at most JOBS (8) compile jobs. No path of the machine that builds
# it goes into the binary: rustc's panic locations in dependencies (cargo's
# registry and git checkouts) and in this tree are rewritten to relative ones,
# so checkouts of the same commit in different folders build byte-identical
# binaries.

JOBS       ?= 8
PREFIX     ?= $(HOME)/.local
REPOS      ?=
BIN        := target/release/xtf
CARGO_HOME ?= $(HOME)/.cargo
RUST_REMAP := --remap-path-prefix=$(CARGO_HOME)=cargo --remap-path-prefix=$(CURDIR)=.

.PHONY: all build check install install-claude calibrate

all: build

build:
	RUSTFLAGS="$$RUSTFLAGS $(RUST_REMAP)" cargo build --release -j $(JOBS)

check:
	cargo test --release -j $(JOBS)
	cargo clippy --release -j $(JOBS) --all-targets -- -D warnings

install: build
	install -Dm755 $(BIN) $(PREFIX)/bin/xtf

# Claude Code, for every project: the plugin (MCP server + hooks) copied to
# PLUGIN_HOME with its two binaries, registered as a directory marketplace and
# installed at user scope; the one decision server as a systemd user unit
# (jevbase itself comes from XKS_Jev-like_Model_Base: its release archive, or
# `make install` there, to ~/.local/bin/jevbase). Running it again upgrades
# the files in place; registration is skipped when already done.
PLUGIN_SRC  := integrations/claude-code
PLUGIN_HOME ?= $(HOME)/.local/share/xtf/claude-code
UNIT_DIR    ?= $(HOME)/.config/systemd/user

install-claude: build
	install -Dm755 $(BIN) $(PREFIX)/bin/xtf
	install -Dm644 $(PLUGIN_SRC)/.claude-plugin/marketplace.json $(PLUGIN_HOME)/.claude-plugin/marketplace.json
	install -Dm644 $(PLUGIN_SRC)/plugin/.claude-plugin/plugin.json $(PLUGIN_HOME)/plugin/.claude-plugin/plugin.json
	install -Dm644 $(PLUGIN_SRC)/plugin/.mcp.json $(PLUGIN_HOME)/plugin/.mcp.json
	install -Dm644 $(PLUGIN_SRC)/plugin/hooks/hooks.json $(PLUGIN_HOME)/plugin/hooks/hooks.json
	install -Dm755 target/release/xtf-mcp $(PLUGIN_HOME)/plugin/bin/xtf-mcp
	install -Dm755 target/release/xtf-hook $(PLUGIN_HOME)/plugin/bin/xtf-hook
	install -Dm644 integrations/systemd/jevbase.service $(UNIT_DIR)/jevbase.service
	systemctl --user daemon-reload
	systemctl --user enable --now jevbase.service
	claude plugin marketplace list 2>/dev/null | grep -qw xtf || claude plugin marketplace add $(PLUGIN_HOME)
	claude plugin list 2>/dev/null | grep -q 'xtf@xtf' || claude plugin install xtf@xtf --scope user
	@echo "installed: plugin $(PLUGIN_HOME), unit $(UNIT_DIR)/jevbase.service; new Claude Code sessions load it"

calibrate: build
	@test -n "$(REPOS)" || { echo "make calibrate REPOS=DIR (the folder holding the calibration repositories)"; exit 2; }
	$(BIN) calibrate calibration/file-root.jsonl --repos $(REPOS)
