# Copyright (C) 2026  Ryan.K
# SPDX-License-Identifier: AGPL-3.0-or-later
#
#   make              build the module for this computer
#   make display-page build the display's page: the client in WebAssembly
#   make test         the Rust tests, then the Lisp tests on the built module
#   make compile      byte-compile the Emacs Lisp, warnings failing it

EMACS ?= emacs
CARGO ?= cargo
TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
EMACS_BATCH = $(EMACS) -Q --batch -L lisp

.PHONY: all module display-page test test-rust test-lisp compile clean

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

clean:
	$(CARGO) clean
