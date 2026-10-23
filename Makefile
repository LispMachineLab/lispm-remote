# Copyright (C) 2026  Ryan.K
# SPDX-License-Identifier: AGPL-3.0-or-later
#
#   make              build the module for this computer
#   make display-page build the display's page: the client in WebAssembly
#   make test         the Rust tests, then the Lisp tests on the built module
#   make compile      byte-compile the Emacs Lisp, warnings failing it
#   make page         build the phone's page into web/dist
#   make serve        serve web/dist at http://localhost:8000

EMACS ?= emacs
CARGO ?= cargo
NPM ?= npm
WASM_PACK ?= wasm-pack
PYTHON ?= python3
PORT ?= 8000
TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
EMACS_BATCH = $(EMACS) -Q --batch -L lisp
PAGE = web/dist
RINGS = web/node_modules/@ringsnetwork/rings-node/dist

.PHONY: all module display-page test test-rust test-lisp compile page serve clean

all: module

module:
	$(CARGO) build --release --target $(TARGET) -p lispm-remote-module

# The client, bound by the wasm-bindgen command of the version its
# crate pins (cargo install wasm-bindgen-cli --version 0.2.121).
WASM_BINDGEN ?= wasm-bindgen
WASM = target/wasm32-unknown-unknown/release/lispm_remote_client.wasm

display-page:
	$(CARGO) build --release --target wasm32-unknown-unknown -p lispm-remote-client
	$(WASM_BINDGEN) --target web --no-typescript --out-dir crates/client/page/pkg $(WASM)

test: test-rust test-lisp

# Release, like the module, so the two share their compiled dependencies.
test-rust:
	$(CARGO) test --release --target $(TARGET)

test-lisp: module
	$(EMACS_BATCH) -L test --eval "(setq lispm-remote-module-source 'build)" \
	  -l lispm-remote-node-test -l lispm-remote-test -l lispm-remote-display-test -f ert-run-tests-batch-and-exit

compile:
	$(EMACS_BATCH) --eval '(setq byte-compile-error-on-warn t)' -f batch-byte-compile lisp/*.el
	/bin/rm -f lisp/*.elc

# Rings's browser node builds from its release tag when installed, which
# takes minutes; it is installed again only when the lock file changes.
web/node_modules/.package-lock.json: web/package.json web/package-lock.json
	cd web && $(NPM) ci --no-audit --no-fund

page: web/node_modules/.package-lock.json
	$(WASM_PACK) build crates/web --release --target web --no-pack \
	  --out-dir $(CURDIR)/web/pkg --out-name lispm_remote_web
	$(WASM_PACK) build crates/client --release --target web --no-pack \
	  --out-dir $(CURDIR)/web/pkg --out-name lispm_remote_client
	cd web && $(NPM) run build
	mkdir -p $(PAGE)/pkg $(PAGE)/rings
	cp web/index.html web/style.css web/vendor/jsQR.js $(PAGE)/
	cp web/pkg/lispm_remote_web.js web/pkg/lispm_remote_web_bg.wasm $(PAGE)/pkg/
	cp web/pkg/lispm_remote_client.js web/pkg/lispm_remote_client_bg.wasm $(PAGE)/pkg/
	cp $(RINGS)/rings_node.js $(RINGS)/rings_node_bg.wasm $(PAGE)/rings/

serve: page
	$(PYTHON) -m http.server --bind 127.0.0.1 --directory $(PAGE) $(PORT)

clean:
	$(CARGO) clean
	/bin/rm -rf $(PAGE) web/pkg
