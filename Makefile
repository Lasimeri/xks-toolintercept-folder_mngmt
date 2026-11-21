# xks-toolintercept-folder_mngmt
#
#   make                           build xtf (release)
#   make check                     unit tests + clippy (no model needed)
#   make install [PREFIX=~/.local] copy xtf to $(PREFIX)/bin
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

.PHONY: all build check install calibrate

all: build

build:
	RUSTFLAGS="$$RUSTFLAGS $(RUST_REMAP)" cargo build --release -j $(JOBS)

check:
	cargo test --release -j $(JOBS)
	cargo clippy --release -j $(JOBS) --all-targets -- -D warnings

install: build
	install -Dm755 $(BIN) $(PREFIX)/bin/xtf

calibrate: build
	@test -n "$(REPOS)" || { echo "make calibrate REPOS=DIR (the folder holding the calibration repositories)"; exit 2; }
	$(BIN) calibrate calibration/file-root.jsonl --repos $(REPOS)
