;;; lispm-remote-node-test.el --- Tests of the Rings node in Emacs -*- lexical-binding: t; -*-

;; Copyright (C) 2026  Ryan.K
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

;; Two nodes in one Emacs connect over WebRTC on this computer, the offer
;; and answer passed by hand, and exchange frames.  The module must be
;; built (`make module'); without it, the tests that need it are skipped.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'lispm-remote-node)

(declare-function lispm-remote-core-account-did "lispm-remote-core")
(declare-function lispm-remote-core-namespace "lispm-remote-core")

(defconst lispm-remote-node-test--patience 90
  "Seconds a test waits for the network; a stale connection takes WebRTC
up to about half a minute to give up.")

(defun lispm-remote-node-test--module-p ()
  "Return non-nil when the built module can be loaded."
  (let ((lispm-remote-module-source 'build))
    (and (ignore-errors (lispm-prebuilt-built-file 'lispm-remote-module))
         (file-readable-p (lispm-prebuilt-built-file 'lispm-remote-module))
         (progn (lispm-remote-module-load) t))))

(defun lispm-remote-node-test--wait (predicate &optional what)
  "Process output until PREDICATE returns non-nil, and return its value.
Fail after `lispm-remote-node-test--patience' seconds, naming WHAT was
awaited."
  (let ((deadline (+ (float-time) lispm-remote-node-test--patience))
        value)
    (while (and (not (setq value (funcall predicate)))
                (< (float-time) deadline))
      (accept-process-output nil 0.05))
    (or value (ert-fail (format "no %s in time" (or what "answer from the network"))))))

(defun lispm-remote-node-test--try (node method params)
  "Return (RESULT . ERROR) of the request METHOD with PARAMS on NODE."
  (let (done)
    (lispm-remote-node-request node method params
                               (lambda (result error) (setq done (cons result error))))
    (lispm-remote-node-test--wait (lambda () done) (format "reply to %s" method))))

(defun lispm-remote-node-test--ask (node method params)
  "Return the result of the request METHOD with PARAMS on NODE."
  (let ((outcome (lispm-remote-node-test--try node method params)))
    (when (cdr outcome)
      (ert-fail (format "%s failed: %s" method (cdr outcome))))
    (car outcome)))

(defun lispm-remote-node-test--connected-p (node did)
  "Return non-nil when NODE is connected to the peer DID."
  (seq-some (lambda (peer)
              (and (equal (alist-get 'did peer) did)
                   (equal (alist-get 'state peer) "Connected")))
            (alist-get 'peers (lispm-remote-node-test--ask node "listPeers" nil))))

(defun lispm-remote-node-test--connect (a b)
  "Connect node A to node B, passing the offer and answer by hand.
B refuses A's offers while it still holds a connection to an earlier node
of A's account, as after A went offline, and A refuses to offer while a
refused offer of its own is pending; A offers again until both let go,
as the page does."
  (let ((deadline (+ (float-time) lispm-remote-node-test--patience))
        answer)
    (while (not answer)
      (let* ((offer (lispm-remote-node-test--try
                     a "createOffer" `((did . ,(lispm-remote-node-did b)))))
             (outcome (and (not (cdr offer))
                           (lispm-remote-node-test--try
                            b "answerOffer" `((offer . ,(alist-get 'offer (car offer))))))))
        (cond ((and outcome (not (cdr outcome))) (setq answer (car outcome)))
              ((< (float-time) deadline) (accept-process-output nil 1))
              (t (ert-fail (format "connecting failed: %s" (or (cdr offer) (cdr outcome))))))))
    (lispm-remote-node-test--ask a "acceptAnswer" `((answer . ,(alist-get 'answer answer))))
    (lispm-remote-node-test--wait
     (lambda () (lispm-remote-node-test--connected-p a (lispm-remote-node-did b)))
     "connection")))

(defun lispm-remote-node-test--offline (seconds)
  "Let SECONDS pass, as a device that lost its network does."
  (let ((until (+ (float-time) seconds)))
    (while (< (float-time) until)
      (accept-process-output nil 0.2))))

(ert-deftest lispm-remote-node-test-account-is-kept-private ()
  (skip-unless (lispm-remote-node-test--module-p))
  (let* ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
         (account (lispm-remote-node-account)))
    (unwind-protect
        (progn
          (should (equal (lispm-remote-node-account) account))
          (should (= (file-modes (lispm-remote-node-account-file)) #o600))
          (should (string-prefix-p "0x" (lispm-remote-core-account-did account))))
      (delete-directory lispm-remote-directory t))))

(ert-deftest lispm-remote-node-test-two-nodes-exchange-frames ()
  (skip-unless (lispm-remote-node-test--module-p))
  (let* (to-b to-a sent
         (a (lispm-remote-node-start (lispm-remote-core-generate-account)
                                     (lambda (from frame) (push (cons from frame) to-a))))
         (b (lispm-remote-node-start (lispm-remote-core-generate-account)
                                     (lambda (from frame) (push (cons from frame) to-b)))))
    (unwind-protect
        (progn
          (should (equal (lispm-remote-core-namespace) "lispm-remote/1"))
          (lispm-remote-node-test--connect a b)
          (lispm-remote-node-send a (lispm-remote-node-did b) "\0\1\377to b"
                                  (lambda (ok error) (setq sent (or ok error))))
          (lispm-remote-node-test--wait (lambda () (and sent to-b)))
          (should (eq sent t))
          (should (equal (car to-b) (cons (lispm-remote-node-did a) "\0\1\377to b")))
          (should-not (multibyte-string-p (cdar to-b)))
          (lispm-remote-node-send b (lispm-remote-node-did a) "to a")
          (lispm-remote-node-test--wait (lambda () to-a))
          (should (equal (car to-a) (cons (lispm-remote-node-did b) "to a"))))
      (lispm-remote-node-stop a)
      (lispm-remote-node-stop b))
    (should-not (lispm-remote-node-live-p a))
    (should-not (process-live-p (lispm-remote-node-process a)))))

(ert-deftest lispm-remote-node-test-failures-reach-the-callback ()
  (skip-unless (lispm-remote-node-test--module-p))
  (let* (outcome
         (a (lispm-remote-node-start (lispm-remote-core-generate-account))))
    (unwind-protect
        (progn
          (lispm-remote-node-send a "0x0000000000000000000000000000000000000001" "x"
                                  (lambda (ok error) (setq outcome (list ok error))))
          (lispm-remote-node-test--wait (lambda () outcome))
          (should-not (car outcome))
          (should (stringp (cadr outcome)))
          (should-error (lispm-remote-node-send a "not a did" "x"))
          (setq outcome nil)
          (lispm-remote-node-request a "noSuchMethod" nil
                                     (lambda (result error) (setq outcome (list result error))))
          (lispm-remote-node-test--wait (lambda () outcome))
          (should-not (car outcome))
          (should (stringp (cadr outcome)))
          ;; Its JSON-RPC error has no data.
          (should-not (get-text-property 0 'lispm-remote-node-data (cadr outcome))))
      (lispm-remote-node-stop a))))

(ert-deftest lispm-remote-node-test-a-failure-carries-its-data ()
  "A request that failed with a JSON-RPC error with data, as Rings refuses
a seed of another wire version, gives its callback the message with the
data, parsed, as its text property."
  (let ((node (lispm-remote-node--create))
        outcome)
    (lispm-remote-node--expect node 7 (lambda (result error) (setq outcome (list result error))))
    (lispm-remote-node--dispatch
     node '(failed 7 "Incompatible wire version: ours is 1, the peer's is from before wire versioning."
                   "{\"ours\":1,\"theirs\":null}"))
    (should-not (car outcome))
    (should (string-prefix-p "Incompatible wire version" (cadr outcome)))
    (should (equal (get-text-property 0 'lispm-remote-node-data (cadr outcome))
                   '((ours . 1) (theirs))))
    (lispm-remote-node--expect node 8 (lambda (result error) (setq outcome (list result error))))
    (lispm-remote-node--dispatch node '(failed 8 "Connection not found." nil))
    (should (equal outcome '(nil "Connection not found.")))
    (should-not (get-text-property 0 'lispm-remote-node-data (cadr outcome)))))

(ert-deftest lispm-remote-node-test-a-renewal-is-told ()
  "A node that renewed its session key runs the renewal hook with itself;
one whose renewal failed says so."
  (let* ((node (lispm-remote-node--create))
         (renewed nil)
         (lispm-remote-node-renewed-functions
          (list (lambda (renewing) (push renewing renewed)))))
    (lispm-remote-node--dispatch node '(renewed))
    (should (equal renewed (list node)))
    (let ((warnings nil))
      (cl-letf (((symbol-function 'display-warning)
                 (lambda (type message &rest _) (push (list type message) warnings))))
        (lispm-remote-node--dispatch node '(renewal-failed "the store is locked")))
      (should (equal (caar warnings) 'lispm-remote))
      (should (string-search "the store is locked" (cadar warnings))))
    (should (equal renewed (list node)))))

(ert-deftest lispm-remote-node-test-without-a-pin-download-explains ()
  (let* ((directory (make-temp-file "lispm-remote-pins" t))
         (lispm-remote-module-source 'download)
         (lispm-remote-module-sums-file (expand-file-name "sums.eld" directory))
         (lispm-remote-module-prebuilt-directory (expand-file-name "prebuilt" directory)))
    (unwind-protect
        (let ((error (should-error (lispm-remote-module-file) :type 'user-error)))
          (should (string-search "no module is pinned for these sources" (cadr error))))
      (delete-directory directory t))))

(defun lispm-remote-node-test--release (directory module)
  "Lay out in DIRECTORY a release of these sources holding MODULE, the
module built here, as the workflow publishes it; return its asset."
  (let* ((target (lispm-prebuilt-target 'lispm-remote-module))
         (asset (lispm-prebuilt-asset 'lispm-remote-module target))
         (release (expand-file-name (format "module-%s" (lispm-remote-module-source-version))
                                    directory)))
    (make-directory release t)
    (should (zerop (call-process-shell-command
                    (format "gzip -9 -n -c %s > %s" (shell-quote-argument module)
                            (shell-quote-argument (expand-file-name asset release))))))
    (should (zerop (call-process-shell-command
                    (format "cd %s && shasum -a 256 %s > SHA256SUMS || sha256sum %s > SHA256SUMS"
                            (shell-quote-argument release) asset asset))))
    (expand-file-name asset release)))

(ert-deftest lispm-remote-node-test-a-published-module-is-pinned-downloaded-and-checked ()
  (skip-unless (and (lispm-remote-node-test--module-p) (executable-find "curl")
                    (executable-find "gzip")))
  (let* ((directory (make-temp-file "lispm-remote-release" t))
         (module (lispm-prebuilt-built-file 'lispm-remote-module))
         (target (lispm-prebuilt-target 'lispm-remote-module))
         (lispm-remote-module-targets (list target))
         (lispm-remote-module-prebuilt-url (concat "file://" directory))
         (lispm-remote-module-sums-file (expand-file-name "sums.eld" directory))
         (lispm-remote-module-prebuilt-directory (expand-file-name "prebuilt" directory))
         (asset (lispm-remote-node-test--release directory module))
         (version (lispm-remote-module-source-version)))
    (unwind-protect
        (progn
          ;; Pinned from the release as `make pin' pins it.
          (with-temp-file lispm-remote-module-sums-file
            (insert (lispm-prebuilt-render-pins
                     'lispm-remote-module version (lispm-remote-module-published lispm-remote-module-prebuilt-url
                                                            version))))
          (should (equal (lispm-prebuilt-pinned-sha256 'lispm-remote-module)
                         (lispm-prebuilt-sha256 asset)))
          ;; Downloaded, checked, unpacked: the very module built here.
          (let ((file (lispm-remote-module-download)))
            (should (string-prefix-p lispm-remote-module-prebuilt-directory file))
            (should (equal (lispm-prebuilt-sha256 file) (lispm-prebuilt-sha256 module)))
            (let ((lispm-remote-module-source 'download))
              (should (equal (lispm-remote-module-file) file))))
          ;; A module changed on the release after it was pinned is refused.
          (delete-directory lispm-remote-module-prebuilt-directory t)
          (with-temp-file asset
            (set-buffer-multibyte nil)
            (insert "not the module"))
          (let ((error (should-error (lispm-remote-module-download))))
            (should (string-search "SHA-256 differs" (cadr error))))
          (should-not (file-exists-p (lispm-prebuilt-prebuilt-file 'lispm-remote-module))))
      (delete-directory directory t))))

(ert-deftest lispm-remote-node-test-a-release-missing-a-computer-is-not-pinned ()
  (skip-unless (and (lispm-remote-node-test--module-p) (executable-find "curl")
                    (executable-find "gzip")))
  (let* ((directory (make-temp-file "lispm-remote-release" t))
         (lispm-remote-module-targets (cons "x86_64-unknown-freebsd" lispm-remote-module-targets)))
    (unwind-protect
        (progn
          (lispm-remote-node-test--release directory (lispm-prebuilt-built-file 'lispm-remote-module))
          (let ((error (should-error (lispm-remote-module-published
                                      (concat "file://" directory)
                                      (lispm-remote-module-source-version)))))
            (should (string-search "has no module for" (cadr error)))))
      (delete-directory directory t))))

(ert-deftest lispm-remote-node-test-targets ()
  (should (equal (lispm-prebuilt-target 'lispm-remote-module "aarch64-apple-darwin24.1.0") "aarch64-apple-darwin"))
  (should (equal (lispm-prebuilt-target 'lispm-remote-module "x86_64-pc-linux-gnu") "x86_64-unknown-linux-gnu"))
  (should (equal (lispm-prebuilt-target 'lispm-remote-module "aarch64-unknown-linux-gnu") "aarch64-unknown-linux-gnu"))
  (should-not (lispm-prebuilt-target 'lispm-remote-module "riscv64-unknown-linux-gnu")))

(provide 'lispm-remote-node-test)
;;; lispm-remote-node-test.el ends here
