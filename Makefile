# Copyright (C) 2026  Ryan.K
# SPDX-License-Identifier: AGPL-3.0-or-later
#
#   make              build the module for this computer
#   make test         the Rust tests, then the Lisp tests on the built module
#   make compile      byte-compile the Emacs Lisp, warnings failing it

EMACS ?= emacs
CARGO ?= cargo
TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
EMACS_BATCH = $(EMACS) -Q --batch -L lisp

.PHONY: all module test test-rust test-lisp compile clean

all: module

module:
	$(CARGO) build --release --target $(TARGET) -p lispm-remote-module

test: test-rust test-lisp

# Release, like the module, so the two share their compiled dependencies.
test-rust:
	$(CARGO) test --release --target $(TARGET)

test-lisp: module
	$(EMACS_BATCH) -L test --eval "(setq lispm-remote-module-source 'build)" \
	  -l lispm-remote-node-test -l lispm-remote-test -f ert-run-tests-batch-and-exit

compile:
	$(EMACS_BATCH) --eval '(setq byte-compile-error-on-warn t)' -f batch-byte-compile lisp/*.el
	/bin/rm -f lisp/*.elc

clean:
	$(CARGO) clean
