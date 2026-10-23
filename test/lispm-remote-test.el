;;; lispm-remote-test.el --- Tests of pairing a phone with LispM -*- lexical-binding: t; -*-

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

;; `lispm-remote-mode' against a phone, all in one Emacs over real Rings
;; nodes: the phone is a second node driving the device's machine, as the
;; page at lispm.ai/remote will.  LispM and the phone are each connected
;; to a third node only, which stands for the network.  The page itself,
;; its camera and a real network outage are the browser test's.

;;; Code:

(require 'ert)
(require 'lispm-remote)
(require 'lispm-remote-node-test)

(declare-function lispm-remote-core-generate-account "lispm-remote-core")
(declare-function lispm-remote-core-generate-static "lispm-remote-core")
(declare-function lispm-remote-core-initiator "lispm-remote-core")
(declare-function lispm-remote-core-initiator-pair "lispm-remote-core")
(declare-function lispm-remote-core-initiator-connect "lispm-remote-core")
(declare-function lispm-remote-core-initiator-receive "lispm-remote-core")
(declare-function lispm-remote-core-initiator-send "lispm-remote-core")
(declare-function lispm-remote-core-initiator-tick "lispm-remote-core")
(declare-function lispm-remote-core-initiator-open-p "lispm-remote-core")

(cl-defstruct (lispm-remote-test-phone (:constructor lispm-remote-test--phone-create)
                                       (:copier nil))
  "A phone: a Rings node and the device's machine, with what it saw."
  account secret store node machine timer pairing fingerprint states failures
  unpaired send-errors asking asked-at)

(defun lispm-remote-test--phone-start (phone)
  "Start PHONE's node with its account, and its machine from its stored
pairing, as when the page loads."
  (let ((node (lispm-remote-node-start
               (lispm-remote-test-phone-account phone)
               (lambda (from frame)
                 (lispm-remote-test--phone-apply
                  phone (lispm-remote-core-initiator-receive
                         (lispm-remote-test-phone-machine phone) from frame)))
               (lispm-remote-test-phone-store phone))))
    (setf (lispm-remote-test-phone-node phone) node
          (lispm-remote-test-phone-machine phone)
          (lispm-remote-core-initiator
           (lispm-remote-test-phone-secret phone) (lispm-remote-node-did node)
           (when-let* ((pairing (lispm-remote-test-phone-pairing phone)))
             (vector (car pairing) (cdr pairing))))
          (lispm-remote-test-phone-timer phone)
          (run-at-time 0.5 0.5 #'lispm-remote-test--phone-tick phone))))

(defun lispm-remote-test--phone-stop (phone)
  "Stop PHONE's node without a word, as when its network drops."
  (cancel-timer (lispm-remote-test-phone-timer phone))
  (lispm-remote-node-stop (lispm-remote-test-phone-node phone)))

(defun lispm-remote-test--phone-tick (phone)
  "Let PHONE's machine send a waiting frame again, and ask for the state
again while none came since the channel opened, as the page does."
  (when (lispm-remote-node-live-p (lispm-remote-test-phone-node phone))
    (let ((machine (lispm-remote-test-phone-machine phone)))
      (lispm-remote-test--phone-apply phone (lispm-remote-core-initiator-tick machine))
      (when (and (lispm-remote-test-phone-asking phone)
                 (lispm-remote-core-initiator-open-p machine)
                 (< (or (lispm-remote-test-phone-asked-at phone) 0) (- (float-time) 2)))
        (setf (lispm-remote-test-phone-asked-at phone) (float-time))
        (lispm-remote-test--phone-apply
         phone (lispm-remote-core-initiator-send machine (json-serialize '((type . "state?")))))))))

(defun lispm-remote-test--phone-apply (phone effects)
  "Carry out EFFECTS of PHONE's machine, as the page would."
  (dolist (effect effects)
    (pcase effect
      (`(transmit ,to ,frame)
       (lispm-remote-node-send (lispm-remote-test-phone-node phone) to frame
                               (lambda (_sent error)
                                 (when error
                                   (push (format "%.1f %s" (float-time) error)
                                         (lispm-remote-test-phone-send-errors phone))))))
      (`(show-fingerprint ,fingerprint)
       (setf (lispm-remote-test-phone-fingerprint phone) fingerprint))
      (`(paired ,responder ,key)
       (setf (lispm-remote-test-phone-pairing phone) (cons responder key)))
      (`(opened ,_channel)
       (setf (lispm-remote-test-phone-asking phone) t
             (lispm-remote-test-phone-asked-at phone) nil))
      (`(unpaired)
       (setf (lispm-remote-test-phone-pairing phone) nil
             (lispm-remote-test-phone-unpaired phone) t))
      (`(delivered ,payload)
       (setf (lispm-remote-test-phone-asking phone) nil)
       (push (json-parse-string (decode-coding-string payload 'utf-8)
                                :object-type 'alist :array-type 'list)
             (lispm-remote-test-phone-states phone)))
      (`(failed . ,why)
       (push why (lispm-remote-test-phone-failures phone))))))

(defun lispm-remote-test--agent-state (phone)
  "Return the state of the first agent in the last state PHONE received."
  (let* ((message (car (lispm-remote-test-phone-states phone)))
         (state (alist-get 'state message)))
    (should (equal (alist-get 'type message) "state"))
    (alist-get 'state (car (alist-get 'agents state)))))

(defvar lispm-remote-test--state nil
  "The state LispM shows, in the tests.")

(defmacro lispm-remote-test--with-world (bindings &rest body)
  "Run BODY with LispM Remote on, a relay node and a phone connected to
it, bound as BINDINGS says: (RELAY PHONE)."
  (declare (indent 1))
  (let ((relay (car bindings)) (phone (cadr bindings)))
    `(let* ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
            (lispm-remote-seeds nil)
            (lispm-remote-check-seconds 1)
            (lispm-remote-test--state '((agents . [((name . "coder") (state . "idle"))])))
            (lispm-remote-state-function (lambda () lispm-remote-test--state))
            (,relay (lispm-remote-node-start (lispm-remote-core-generate-account)))
            (,phone (lispm-remote-test--phone-create
                     :account (lispm-remote-core-generate-account)
                     :secret (lispm-remote-core-generate-static)
                     ;; The page keeps its replay records in IndexedDB.
                     :store (expand-file-name "phone" lispm-remote-directory))))
       (unwind-protect
           (progn
             (lispm-remote-mode 1)
             (lispm-remote-node-test--connect lispm-remote--node ,relay)
             (lispm-remote-test--phone-start ,phone)
             (lispm-remote-node-test--connect (lispm-remote-test-phone-node ,phone) ,relay)
             ,@body)
         (lispm-remote-mode -1)
         (lispm-remote-node-stop ,relay)
         (when (lispm-remote-test-phone-node ,phone)
           (lispm-remote-test--phone-stop ,phone))
         (when (get-buffer lispm-remote--buffer)
           (kill-buffer lispm-remote--buffer))
         (delete-directory lispm-remote-directory t)))))

(defun lispm-remote-test--claim (phone)
  "Show a pairing code, have PHONE scan it, and wait for LispM to ask the
owner.  Return the fingerprint the phone shows."
  (lispm-remote-pair)
  (lispm-remote-test--phone-apply
   phone (lispm-remote-core-initiator-pair (lispm-remote-test-phone-machine phone)
                                           (plist-get lispm-remote--offer :token)
                                           "Test Phone"))
  (lispm-remote-node-test--wait (lambda () (plist-get lispm-remote--offer :fingerprint))
                                "pairing request")
  (lispm-remote-test-phone-fingerprint phone))

(ert-deftest lispm-remote-test-pair-see-state-drop-and-reconnect ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (relay phone)
    ;; The code Emacs shows, and the same fingerprint on both screens.
    (let ((fingerprint (lispm-remote-test--claim phone)))
      (should (equal (plist-get lispm-remote--offer :fingerprint) fingerprint))
      (with-current-buffer lispm-remote--buffer
        (should (string-search fingerprint (buffer-string))))
      (lispm-remote-approve fingerprint))
    ;; Paired: both ends keep the other, and the phone sees LispM's state.
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-states phone))
                                  "state after pairing")
    (should (equal (lispm-remote-test--agent-state phone) "idle"))
    (should (equal (mapcar (lambda (device) (plist-get device :name)) (lispm-remote--devices))
                   '("Test Phone")))
    (should (equal (car (lispm-remote-test-phone-pairing phone))
                   (lispm-remote-node-did lispm-remote--node)))
    (with-current-buffer lispm-remote--buffer
      (should (string-search "Paired with Test Phone" (buffer-string))))
    ;; LispM's state changes while the phone is connected.
    (setq lispm-remote-test--state '((agents . [((name . "coder") (state . "requesting"))])))
    (lispm-remote-state-changed)
    (lispm-remote-node-test--wait
     (lambda () (equal (lispm-remote-test--agent-state phone) "requesting"))
     "changed state")
    ;; The phone goes offline for a while, and the state changes meanwhile.
    ;; Its node is gone without a word, as when the network drops; the
    ;; relay notices after about five seconds.
    (lispm-remote-test--phone-stop phone)
    (setq lispm-remote-test--state '((agents . [((name . "coder") (state . "acting"))])))
    (lispm-remote-state-changed)
    (lispm-remote-node-test--offline 8)
    ;; Back online: the page loads again, with its account and pairing,
    ;; joins the network, and opens a session without scanning.
    (setf (lispm-remote-test-phone-states phone) nil)
    (lispm-remote-test--phone-start phone)
    (lispm-remote-node-test--connect (lispm-remote-test-phone-node phone) relay)
    (lispm-remote-test--phone-apply
     phone (lispm-remote-core-initiator-connect (lispm-remote-test-phone-machine phone)))
    (ert-info ((lambda ()
                 (format "phone send errors: %S\nLispM log:\n%s"
                         (lispm-remote-test-phone-send-errors phone)
                         (with-current-buffer (get-buffer-create lispm-remote--log-buffer)
                           (buffer-string)))))
      (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-states phone))
                                    "state after reconnecting"))
    (should (equal (lispm-remote-test--agent-state phone) "acting"))))

(ert-deftest lispm-remote-test-a-refused-phone-is-told-and-not-kept ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay phone)
    (lispm-remote-test--claim phone)
    (lispm-remote-refuse)
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-failures phone)))
    (should (equal (car (lispm-remote-test-phone-failures phone)) '(refused owner-refused)))
    (should (eq (plist-get lispm-remote--offer :ended) 'refused))
    (should-not (lispm-remote--devices))
    (should-not (lispm-remote-test-phone-pairing phone))))

(ert-deftest lispm-remote-test-a-wrong-fingerprint-pairs-nobody ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay phone)
    (lispm-remote-test--claim phone)
    (should-error (lispm-remote-approve "0000-0000"))
    (should-not (lispm-remote--devices))
    (should (plist-get lispm-remote--offer :fingerprint))))

(ert-deftest lispm-remote-test-a-forgotten-phone-is-turned-away ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (relay phone)
    (lispm-remote-approve (lispm-remote-test--claim phone))
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-pairing phone)))
    (lispm-remote-forget (lispm-remote-node-did (lispm-remote-test-phone-node phone)))
    (should-not (lispm-remote--devices))
    ;; The phone comes back and is told it is no longer paired.
    (lispm-remote-test--phone-stop phone)
    (lispm-remote-node-test--offline 8)
    (lispm-remote-test--phone-start phone)
    (lispm-remote-node-test--connect (lispm-remote-test-phone-node phone) relay)
    (lispm-remote-test--phone-apply
     phone (lispm-remote-core-initiator-connect (lispm-remote-test-phone-machine phone)))
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-unpaired phone)))
    (should-not (lispm-remote-test-phone-pairing phone))))

(ert-deftest lispm-remote-test-keys-are-kept-private ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay _phone)
    (let ((static (lispm-remote--static)))
      (should (equal (lispm-remote--static) static))
      (should (= (file-modes (lispm-remote--file "static")) #o600))
      (should (= (file-modes lispm-remote-directory) #o700)))))

(ert-deftest lispm-remote-test-the-code-is-drawn-for-cameras-and-terminals ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay _phone)
    (lispm-remote-pair)
    (let* ((rows (lispm-remote--qr-rows))
           (side (length rows))
           (file (make-temp-file "lispm-remote-qr" nil ".pbm")))
      (unwind-protect
          (progn
            (should (cl-every (lambda (row) (= (length row) side)) rows))
            (lispm-remote-write-qr file 2)
            (with-temp-buffer
              (insert-file-contents file)
              (should (looking-at (format "P1\n%d %d\n" (* 2 side) (* 2 side))))
              (should (= (count-lines (point-min) (point-max)) (+ 2 (* 2 side))))))
        (delete-file file))
      (should (= (length (split-string (lispm-remote--qr-text rows) "\n"))
                 (/ (1+ side) 2))))))

;;; lispm-remote-test.el ends here
