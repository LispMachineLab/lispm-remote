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
;; to build it or say where it is.  The one the repository's module
;; workflow built for this computer from these sources is downloaded, and
;; kept only when its SHA-256 is the one pinned beside this file, in
;; lispm-remote-module-sums.eld.  Those checksums come with the Lisp, from
;; the repository's signed history, not from the release, so a module
;; changed on the release after it was pinned is refused.
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

;;; Code:

(require 'seq)
(require 'subr-x)

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

(defconst lispm-remote-module-targets
  '("aarch64-apple-darwin" "x86_64-apple-darwin" "aarch64-unknown-linux-gnu"
    "x86_64-unknown-linux-gnu")
  "The computers a module is published for.")


(defun lispm-remote-module--target (&optional configuration)
  "Return the Rust target for a computer Emacs was built for as
CONFIGURATION, by default this one's `system-configuration', or nil
when the module is not made for it."
  (let ((configuration (or configuration system-configuration)))
    (cond ((string-match-p "\\`\\(?:aarch64\\|arm64\\)-apple-darwin" configuration) "aarch64-apple-darwin")
          ((string-match-p "\\`x86_64-apple-darwin" configuration) "x86_64-apple-darwin")
          ((string-match-p "\\`x86_64-[^-]+-linux-gnu\\'" configuration) "x86_64-unknown-linux-gnu")
          ((string-match-p "\\`aarch64-[^-]+-linux-gnu\\'" configuration) "aarch64-unknown-linux-gnu"))))

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
  "Return the files the module is built from, sorted."
  (let ((root lispm-remote-module-root))
    (sort
     (seq-filter
      #'file-regular-p
     (append (mapcar (lambda (name) (expand-file-name name root)) '("Cargo.toml" "Cargo.lock"))
             (mapcan (lambda (crate)
                       (let ((directory (expand-file-name crate root)))
                         (and (file-directory-p directory)
                              (cons (expand-file-name "Cargo.toml" directory)
                                    (directory-files-recursively
                                     (expand-file-name "src" directory) "\\.rs\\'")))))
                     '("crates/display" "crates/module" "crates/protocol" "crates/relay"))))
     #'string<)))

(defun lispm-remote-module--sha256 (file)
  "Return the SHA-256 of FILE's bytes, in hex."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (secure-hash 'sha256 (current-buffer))))

(defun lispm-remote-module-source-digest ()
  "Return the SHA-256 of the module's sources, in hex, or nil without them.
It is that of what sha256sum writes for them at the workspace's root: for
each, in order, a line of its SHA-256, two spaces and its name."
  (when-let* ((files (lispm-remote-module--sources)))
    (secure-hash 'sha256
                 (mapconcat (lambda (file)
                              (format "%s  %s\n" (lispm-remote-module--sha256 file)
                                      (file-relative-name file lispm-remote-module-root)))
                            files ""))))

(defun lispm-remote-module-source-version ()
  "Return the version of the module's sources, or nil without them: the
first 16 digits of `lispm-remote-module-source-digest'.  A prebuilt module
is published, pinned and kept under it."
  (when-let* ((digest (lispm-remote-module-source-digest)))
    (substring digest 0 16)))

;;;; Pinned checksums

(defvar lispm-remote-module-sums-file
  (expand-file-name "lispm-remote-module-sums.eld" lispm-remote-module-lisp-directory)
  "The file pinning the SHA-256 of the modules to download.")

(defvar lispm-remote-module-prebuilt-directory
  (expand-file-name "prebuilt" lispm-remote-module-root)
  "The directory downloaded modules are kept in, by version.")

(defun lispm-remote-module--sums-file ()
  "Return the file pinning the SHA-256 of the modules to download."
  lispm-remote-module-sums-file)

(defun lispm-remote-module--pins ()
  "Return what lispm-remote-module-sums.eld pins: a plist whose :version
is the version of the sources pinned and :modules an alist of a SHA-256
for each target; nil when nothing is pinned."
  (let ((file (lispm-remote-module--sums-file)))
    (when (file-readable-p file)
      (with-temp-buffer
        (insert-file-contents file)
        (let ((pins (ignore-errors (read (current-buffer)))))
          (and (listp pins) pins))))))

(defun lispm-remote-module--pinned-sha256 (&optional target)
  "Return the SHA-256 pinned for the module of these sources for TARGET,
by default this computer's, or nil when none is pinned for them."
  (when-let* ((target (or target (lispm-remote-module--target)))
              (version (lispm-remote-module-source-version))
              (pins (lispm-remote-module--pins))
              ((equal (plist-get pins :version) version))
              (sha256 (cdr (assoc target (plist-get pins :modules)))))
    (and (stringp sha256) (string-match-p "\\`[0-9a-f]\\{64\\}\\'" sha256) sha256)))

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

;;;; Downloading, checked

(defun lispm-remote-module--asset (target)
  "Return the name the module for TARGET is published under, compressed."
  (format "lispm-remote-module-%s%s.gz" target (lispm-remote-module--suffix target)))

(defun lispm-remote-module--prebuilt-directory ()
  "Return the directory downloaded modules are kept in, by version."
  lispm-remote-module-prebuilt-directory)

(defun lispm-remote-module--prebuilt-file ()
  "Return where the module downloaded for the sources is kept, whether it
is there or not, or nil when none is published for this computer or there
are no sources to name its version."
  (when-let* ((target (lispm-remote-module--target))
              (version (lispm-remote-module-source-version)))
    (expand-file-name (format "lispm-remote-module-%s%s" target (lispm-remote-module--suffix target))
                      (expand-file-name version (lispm-remote-module--prebuilt-directory)))))

(defun lispm-remote-module--why-not ()
  "Return why no module can be downloaded, a sentence."
  (let ((version (lispm-remote-module-source-version)))
    (cond ((not (lispm-remote-module--target))
           (format "no module is published for %s" system-configuration))
          ((not version) "the module's sources, which name the module to download, are not there")
          ((not (lispm-remote-module--pinned-sha256))
           (format "no module is pinned for these sources (version %s); set `lispm-remote-module-source' to `build' to build one"
                   version))
          ((not (executable-find "curl")) "downloading the module needs curl")
          ((not (executable-find "gzip")) "unpacking the module needs gzip")
          (t "no module is there"))))

(defun lispm-remote-module--fetch (url file)
  "Download URL into FILE with curl, or signal an error."
  (unless (zerop (call-process "curl" nil nil nil "--fail" "--silent" "--show-error" "--location"
                               "--retry" "2" "--max-time" "300" "--output" file url))
    (error "Could not download %s" url)))

(defun lispm-remote-module-download ()
  "Download the module built for this computer from the sources there are,
Emacs waiting, check it against the SHA-256 pinned for it, and keep it;
return it, or signal why there is none."
  (interactive)
  (let ((file (lispm-remote-module--prebuilt-file))
        (sha256 (lispm-remote-module--pinned-sha256)))
    (unless (and file sha256 (executable-find "curl") (executable-find "gzip"))
      (user-error "No module of LispM Remote can be downloaded: %s"
                  (lispm-remote-module--why-not)))
    (let* ((asset (lispm-remote-module--asset (lispm-remote-module--target)))
           (url (format "%s/module-%s/%s"
                        (string-trim-right lispm-remote-module-prebuilt-url "/+")
                        (lispm-remote-module-source-version) asset))
           (directory (make-temp-file "lispm-remote" t))
           (compressed (expand-file-name asset directory))
           (unpacked (file-name-sans-extension compressed)))
      (unwind-protect
          (progn
            (message "Downloading LispM Remote's module...")
            (lispm-remote-module--fetch url compressed)
            (unless (equal (lispm-remote-module--sha256 compressed) sha256)
              (error "%s is not the module pinned for these sources: its SHA-256 differs, so it was refused"
                     asset))
            (unless (zerop (call-process "gzip" nil nil nil "-d" "-f" compressed))
              (error "%s could not be unpacked" asset))
            (make-directory (file-name-directory file) t)
            ;; Renamed into place, so the module is there whole or not at all.
            (copy-file unpacked (concat file ".part") t)
            (rename-file (concat file ".part") file t)
            (dolist (old (directory-files (lispm-remote-module--prebuilt-directory) t "\\`[^.]"))
              (when (and (file-directory-p old)
                         (not (string= old (directory-file-name (file-name-directory file)))))
                (delete-directory old t)))
            (message "Downloading LispM Remote's module...done")
            file)
        (delete-directory directory t)))))

;;;; Loading

(defun lispm-remote-module-file ()
  "Return the module to load, getting it first when it is not there:
built with Cargo, again when a source is newer, when
`lispm-remote-module-source' is `build'; else downloaded and checked.
Signal an error when there is none to be had."
  (pcase lispm-remote-module-source
    ('build
     (let ((built (lispm-remote-module--built-file)))
       (if (and built (file-readable-p built) (not (lispm-remote-module--stale-p built)))
           built
         (lispm-remote-module-build))))
    (_
     (let ((prebuilt (lispm-remote-module--prebuilt-file)))
       (if (and prebuilt (file-readable-p prebuilt))
           prebuilt
         (lispm-remote-module-download))))))

(defun lispm-remote-module-load ()
  "Load LispM Remote's module unless it is loaded."
  (unless (featurep 'lispm-remote-core)
    (let ((file (lispm-remote-module-file)))
      (module-load file)
      (unless (featurep 'lispm-remote-core)
        (error "%s is not LispM Remote's module" file)))))

;;;; In batch: a workflow's

(defun lispm-remote-module-published (url version)
  "Return the alist of target and SHA-256 of the release of VERSION under
URL, each module downloaded and found to be the one its SHA256SUMS lists,
and one there for every target in `lispm-remote-module-targets'.  Signal
an error otherwise."
  (let* ((release (format "%s/module-%s/" (string-trim-right url "/+") version))
         (directory (make-temp-file "lispm-remote-pin" t))
         (sums (expand-file-name "SHA256SUMS" directory))
         (modules nil))
    (unwind-protect
        (progn
          (lispm-remote-module--fetch (concat release "SHA256SUMS") sums)
          (dolist (line (split-string (with-temp-buffer (insert-file-contents sums) (buffer-string))
                                      "\n" t "[ \t\r]+"))
            (unless (string-match "\\`\\([0-9a-f]\\{64\\}\\) [ *]lispm-remote-module-\\(.+?\\)\\.\\(?:dylib\\|so\\)\\.gz\\'" line)
              (error "SHA256SUMS of module-%s lists %S, not a module" version line))
            (let* ((sha256 (match-string 1 line))
                   (target (match-string 2 line))
                   (file (expand-file-name (lispm-remote-module--asset target) directory)))
              (lispm-remote-module--fetch (concat release (lispm-remote-module--asset target)) file)
              (unless (equal (lispm-remote-module--sha256 file) sha256)
                (error "%s is not the module SHA256SUMS lists: refusing to pin it"
                       (lispm-remote-module--asset target)))
              (push (cons target sha256) modules)))
          (let ((missing (seq-remove (lambda (target) (assoc target modules))
                                     lispm-remote-module-targets)))
            (when missing
              (error "module-%s has no module for %s" version (string-join missing ", "))))
          (sort modules (lambda (a b) (string< (car a) (car b)))))
      (delete-directory directory t))))

(defun lispm-remote-module--render-pins (version modules)
  "Return the text of lispm-remote-module-sums.eld pinning MODULES of VERSION."
  (concat ";;; lispm-remote-module-sums.eld --- The modules LispM Remote downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as
;; published, compressed: the module lispm-remote-module.el downloads for
;; these sources.  A module whose SHA-256 is not the one here is refused.
;; VERSION must be the version of the module's sources.
;; `lispm-remote-module-batch-pin' writes it once the module workflow has
;; published that release.

"
          (format "(:version %S\n :modules (%s))\n" version
                  (mapconcat (lambda (module) (format "(%S . %S)" (car module) (cdr module)))
                             modules "\n           "))))

(defun lispm-remote-module-batch-pin ()
  "Pin the published modules of these sources, for Emacs in batch.
It writes lispm-remote-module-sums.eld from the release module-VERSION
under `lispm-remote-module-prebuilt-url', each module checked against the
release's SHA256SUMS; run it once the workflow has published the release,
and commit the file with the release of LispM Remote that ships it.  With
LISPM_REMOTE_PIN_CHECK set it writes nothing and exits with 1 when the
file pins these sources otherwise than the release has them; a file
pinning other sources passes, with a notice, as it does between a change
of the sources and the next release."
  (let ((version (or (lispm-remote-module-source-version) (error "No sources here")))
        (pins (lispm-remote-module--pins))
        (file (lispm-remote-module--sums-file)))
    (condition-case error
        (if (getenv "LISPM_REMOTE_PIN_CHECK")
            (cond ((not (equal (plist-get pins :version) version))
                   (message "::notice::%s pins %s, not the sources' %s: pin it once module-%s is published"
                            (file-name-nondirectory file)
                            (if (plist-get pins :version)
                                (format "module-%s" (plist-get pins :version))
                              "nothing")
                            version version))
                  ((equal (lispm-remote-module-published lispm-remote-module-prebuilt-url version)
                          (sort (copy-sequence (plist-get pins :modules))
                                (lambda (a b) (string< (car a) (car b)))))
                   (message "%s pins module-%s as published" (file-name-nondirectory file) version))
                  (t (error "%s differs from the release module-%s"
                            (file-name-nondirectory file) version)))
          (with-temp-file file
            (insert (lispm-remote-module--render-pins
                     version (lispm-remote-module-published lispm-remote-module-prebuilt-url version))))
          (message "%s pins module-%s" (file-name-nondirectory file) version))
      (error (message "%s" (error-message-string error))
             (kill-emacs 1)))
    (kill-emacs 0)))

(provide 'lispm-remote-module)
;;; lispm-remote-module.el ends here
