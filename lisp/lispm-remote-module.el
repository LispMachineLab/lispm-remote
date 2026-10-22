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

;; LispM Remote's Rings Network node runs in a Rust module, the crate in
;; crates/module.  Users are meant to get it the way they get LispM's
;; other modules: downloaded for their computer and kept only when its
;; checksum is the one pinned beside the Lisp.  No module is published
;; yet, so for now it comes only from Cargo, by choice:
;;
;;   (setq lispm-remote-module-source 'build)
;;
;; builds it in this checkout (`make module' does the same) and loads it,
;; building again whenever a source is newer than the module.

;;; Code:

(require 'seq)

(defgroup lispm-remote nil
  "Reach a running LispM from a phone."
  :group 'comm
  :prefix "lispm-remote-")

(defcustom lispm-remote-module-source 'download
  "Where LispM Remote's module comes from.
`download' takes the one published for these sources, checked against
its pinned checksum; `build' builds it here with Cargo."
  :type '(choice (const :tag "Downloaded and checked" download)
                 (const :tag "Built here with Cargo" build)))

(defconst lispm-remote-module-lisp-directory
  (file-name-directory (or load-file-name buffer-file-name))
  "Directory of LispM Remote's Lisp.")

(defconst lispm-remote-module-root
  (file-name-directory (directory-file-name lispm-remote-module-lisp-directory))
  "The checkout: the Cargo workspace's directory.")

(defconst lispm-remote-module-targets
  '("aarch64-apple-darwin" "x86_64-apple-darwin" "x86_64-unknown-linux-gnu")
  "The computers the module is made for.")


(defun lispm-remote-module--target (&optional configuration)
  "Return the Rust target for a computer Emacs was built for as
CONFIGURATION, by default this one's `system-configuration', or nil
when the module is not made for it."
  (let ((configuration (or configuration system-configuration)))
    (cond ((string-match-p "\\`\\(?:aarch64\\|arm64\\)-apple-darwin" configuration) "aarch64-apple-darwin")
          ((string-match-p "\\`x86_64-apple-darwin" configuration) "x86_64-apple-darwin")
          ((string-match-p "\\`x86_64-[^-]+-linux-gnu\\'" configuration) "x86_64-unknown-linux-gnu"))))

(defun lispm-remote-module--suffix (target)
  "Return the file name suffix of a module for TARGET."
  (if (string-suffix-p "-apple-darwin" target) ".dylib" ".so"))

(defun lispm-remote-module--built-file ()
  "Return where Cargo builds the module for this computer, or nil."
  (when-let* ((target (lispm-remote-module--target)))
    (expand-file-name (format "target/%s/release/liblispm_remote_module%s"
                              target (lispm-remote-module--suffix target))
                      lispm-remote-module-root)))

(defun lispm-remote-module--sources ()
  "Return the files the module is built from."
  (let ((root lispm-remote-module-root))
    (seq-filter
     #'file-regular-p
     (append (mapcar (lambda (name) (expand-file-name name root)) '("Cargo.toml" "Cargo.lock"))
             (mapcan (lambda (crate)
                       (let ((directory (expand-file-name crate root)))
                         (and (file-directory-p directory)
                              (cons (expand-file-name "Cargo.toml" directory)
                                    (directory-files-recursively
                                     (expand-file-name "src" directory) "\\.rs\\'")))))
                     '("crates/module" "crates/protocol"))))))

(defun lispm-remote-module--stale-p (module)
  "Return non-nil when a source is newer than the built MODULE."
  (seq-some (lambda (file) (file-newer-than-file-p file module))
            (lispm-remote-module--sources)))

(defun lispm-remote-module-build ()
  "Build the module with Cargo and return its file.
Signal an error, showing Cargo's output, when it fails."
  (let ((target (lispm-remote-module--target))
        (default-directory lispm-remote-module-root))
    (unless target
      (error "LispM Remote's module is not made for %s" system-configuration))
    (unless (executable-find "cargo")
      (error "Building LispM Remote's module needs Cargo"))
    (let ((buffer (get-buffer-create "*lispm-remote-module build*")))
      (with-current-buffer buffer (erase-buffer))
      (message "Building LispM Remote's module...")
      (unless (zerop (call-process "cargo" nil buffer nil "build" "--release"
                                   "--target" target "-p" "lispm-remote-module"))
        (display-buffer buffer)
        (error "Cargo could not build LispM Remote's module"))
      (message "Building LispM Remote's module...done")
      (lispm-remote-module--built-file))))

(defun lispm-remote-module-file ()
  "Return the module to load, building it first when it is missing or
stale.  Signal an error when there is none to be had."
  (pcase lispm-remote-module-source
    ('build
     (let ((built (lispm-remote-module--built-file)))
       (if (and built (file-readable-p built) (not (lispm-remote-module--stale-p built)))
           built
         (lispm-remote-module-build))))
    (_ (user-error "No module of LispM Remote is published yet; set `lispm-remote-module-source' to `build'"))))

(defun lispm-remote-module-load ()
  "Load LispM Remote's module unless it is loaded."
  (unless (featurep 'lispm-remote-core)
    (let ((file (lispm-remote-module-file)))
      (module-load file)
      (unless (featurep 'lispm-remote-core)
        (error "%s is not LispM Remote's module" file)))))

(provide 'lispm-remote-module)
;;; lispm-remote-module.el ends here
