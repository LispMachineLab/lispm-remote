;;; lispm-remote-module.el --- LispM Remote's native module -*- lexical-binding: t; -*-

;; Copyright (C) 2026  Ryan.K

;; Author: Ryan.K
;; URL: https://github.com/LispMachineLab/lispm-remote
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; This file is not part of GNU Emacs.

;; This program is free software; you can redistribute it and/or modify
;; it under the terms of the GNU Affero General Public License as published
;; by the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; This program is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU Affero General Public License for more details.

;; You should have received a copy of the GNU Affero General Public License
;; along with this program.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; LispM Remote's Rings Network node runs in a Rust module, and nobody has
;; to build it or say where it is.  lispm-prebuilt downloads the one the
;; repository's module workflow built for this computer from these
;; sources, and keeps it only when its SHA-256 is the one pinned beside
;; this file, in lispm-remote-module-sums.eld.  Those checksums come with
;; the Lisp, from the repository's signed history, not from the release,
;; so a module changed on the release after it was pinned is refused.
;;
;; Cargo builds it only when asked to: `lispm-remote-module-source' set to
;; `build' builds it from the sources there are, and again when one is
;; newer than the module Cargo built, and downloads nothing.
;;
;;   download   prebuilt/VERSION/lispm-remote-module-TARGET.SUFFIX
;;   build      target/TARGET/release/liblispm_remote_module.SUFFIX
;;
;; VERSION is `lispm-remote-module-source-version', a digest of the
;; module's sources: the workspace's manifest and lock file and the crates
;; it is built from.  The workflow publishes the modules of each version,
;; compressed with gzip, as the release module-VERSION;
;; `lispm-remote-module-batch-pin' (`make pin') then writes the SHA-256 of
;; each into lispm-remote-module-sums.eld, in the release of LispM Remote
;; that ships them.
;;
;; Opening LispM Remote has the module downloaded in the background, see
;; `lispm-remote-module-prefetch', through lispm-kernel when it runs; the
;; node, started before it is there, waits for it.

;;; Code:

(require 'lispm-prebuilt)

(defgroup lispm-remote nil
  "Reach a running LispM from a phone."
  :group 'comm
  :prefix "lispm-remote-")

(defcustom lispm-remote-module-source 'download
  "Where LispM Remote's module comes from.
`download', the default, downloads the module the repository's workflow
built for this computer from these sources, and keeps it only when its
SHA-256 is the one pinned in lispm-remote-module-sums.eld.  `build'
builds it with Cargo instead, from the sources there are, and again when
one is newer than the module Cargo built; nothing is downloaded then."
  :type '(choice (const :tag "Download it, checked" download)
                 (const :tag "Build it with Cargo" build)))

(defcustom lispm-remote-module-prebuilt-url
  "https://github.com/LispMachineLab/lispm-remote/releases/download"
  "Where the prebuilt modules are.
The modules of the sources whose version is VERSION, see
`lispm-remote-module-source-version', are under module-VERSION there:
lispm-remote-module-TARGET.SUFFIX.gz for each TARGET.  Whatever the URL,
a module is kept only when its SHA-256 is the one
lispm-remote-module-sums.eld pins."
  :type 'string)

(defconst lispm-remote-module-lisp-directory
  (file-name-directory (or load-file-name buffer-file-name))
  "Directory of LispM Remote's Lisp.")

(defconst lispm-remote-module-root
  (file-name-directory (directory-file-name lispm-remote-module-lisp-directory))
  "The checkout: the Cargo workspace's directory.")

(defvar lispm-remote-module-targets
  '("aarch64-apple-darwin" "x86_64-apple-darwin" "aarch64-unknown-linux-gnu"
    "x86_64-unknown-linux-gnu")
  "The computers a module is published for.")

(defvar lispm-remote-module-sums-file
  (expand-file-name "lispm-remote-module-sums.eld" lispm-remote-module-lisp-directory)
  "The file pinning the SHA-256 of the modules to download.")

(defvar lispm-remote-module-prebuilt-directory
  (expand-file-name "prebuilt" lispm-remote-module-root)
  "The directory downloaded modules are kept in, by version.")

(lispm-prebuilt-define 'lispm-remote-module
  :title "LispM Remote's module"
  :root lispm-remote-module-root
  :sources (append '("Cargo.toml" "Cargo.lock")
                   (mapcan (lambda (crate)
                             (list (concat crate "/Cargo.toml") (list (concat crate "/src") "\\.rs\\'")))
                           '("crates/display" "crates/module" "crates/protocol" "crates/relay")))
  :targets (lambda () lispm-remote-module-targets)
  :url 'lispm-remote-module-prebuilt-url
  :source 'lispm-remote-module-source
  :release "module-%v"
  :asset "lispm-remote-module-%t%e.gz"
  :file "lispm-remote-module-%t%e"
  :prebuilt (lambda () lispm-remote-module-prebuilt-directory)
  :sums (lambda () lispm-remote-module-sums-file)
  :build '("cargo" "build" "--release" "--target" "%t" "-p" "lispm-remote-module")
  :built "target/%t/release/liblispm_remote_module%e"
  :pin-check "LISPM_REMOTE_PIN_CHECK")

(defun lispm-remote-module-source-version ()
  "Return the version of the module's sources, or nil without them: the
first 16 digits of the SHA-256 of what sha256sum writes for them at the
workspace's root.  A prebuilt module is published, pinned and kept
under it."
  (lispm-prebuilt-source-version 'lispm-remote-module))

(defun lispm-remote-module-build ()
  "Build the module with Cargo and return its file.
Signal an error, its output in *lispm-remote-module build*, when it fails."
  (lispm-prebuilt-build 'lispm-remote-module))

(defun lispm-remote-module-download ()
  "Download the module built for this computer from the sources there are,
Emacs waiting, check it against the SHA-256 pinned for it, and keep it;
return it, or signal why there is none."
  (interactive)
  (lispm-prebuilt-download 'lispm-remote-module))

(defun lispm-remote-module-file ()
  "Return the module to load, getting it first when it is not there, Emacs
waiting: built with Cargo, again when a source is newer, when
`lispm-remote-module-source' is `build'; else downloaded and checked.
Signal an error when there is none to be had."
  (lispm-prebuilt-ensure 'lispm-remote-module))

(defun lispm-remote-module-prefetch ()
  "Download the module in the background when it is not there, so that
the node need not wait for it."
  (lispm-prebuilt-prefetch 'lispm-remote-module))

(defun lispm-remote-module-load ()
  "Load LispM Remote's module unless it is loaded."
  (lispm-prebuilt-load 'lispm-remote-module 'lispm-remote-core))

(defun lispm-remote-module-published (url version)
  "Return the alist of target and SHA-256 of the release of VERSION under
URL, as `lispm-prebuilt-published' checks it."
  (lispm-prebuilt-published 'lispm-remote-module url version))

(defun lispm-remote-module-batch-pin ()
  "Pin the published modules of these sources, for Emacs in batch.
See `lispm-prebuilt-batch-pin'; LISPM_REMOTE_PIN_CHECK set asks it only
to check the pins against the release."
  (lispm-prebuilt-batch-pin 'lispm-remote-module))

(provide 'lispm-remote-module)
;;; lispm-remote-module.el ends here
