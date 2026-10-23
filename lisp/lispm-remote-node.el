;;; lispm-remote-node.el --- A Rings Network node inside Emacs -*- lexical-binding: t; -*-

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

;; A Rings Network node running inside Emacs, on the module's own threads,
;; so this Emacs can be reached by its DID from anywhere while it runs.
;; Emacs never waits on the network: work is started here and its outcome
;; arrives later, through the node's pipe process, as callbacks.
;;
;;   (setq node (lispm-remote-node-start))
;;   (lispm-remote-node-connect node "https://node.rings.rs")
;;   (lispm-remote-node-send node DID FRAME)        ; FRAME a unibyte string
;;   (lispm-remote-node-request node "listPeers" nil
;;                              (lambda (result error) ...))
;;
;; Frames other peers send arrive in `lispm-remote-node-frame-functions'
;; and the node's own frame function.  Rings authenticated their sender;
;; what they say is for LispM Remote's protocol to judge.
;;
;; The node's identity is a Rings account key, made the first time and
;; kept, readable only by you, in `lispm-remote-directory'.

;;; Code:

(require 'cl-lib)
(require 'subr-x)
(require 'lispm-remote-module)

(declare-function lispm-remote-core-start "lispm-remote-core")
(declare-function lispm-remote-core-stop "lispm-remote-core")
(declare-function lispm-remote-core-live-p "lispm-remote-core")
(declare-function lispm-remote-core-did "lispm-remote-core")
(declare-function lispm-remote-core-send "lispm-remote-core")
(declare-function lispm-remote-core-request "lispm-remote-core")
(declare-function lispm-remote-core-events "lispm-remote-core")
(declare-function lispm-remote-core-generate-account "lispm-remote-core")

(defcustom lispm-remote-ice-servers "stun://stun.l.google.com:19302"
  "The STUN and TURN servers a node finds its way out through, as Rings
reads them."
  :type 'string
  :group 'lispm-remote)

(defcustom lispm-remote-directory (locate-user-emacs-file "lispm-remote/")
  "Where LispM Remote keeps its keys."
  :type 'directory
  :group 'lispm-remote)

(defvar lispm-remote-node-frame-functions nil
  "Abnormal hook run with NODE, FROM and FRAME for each frame a peer sends.
FROM is the sender's DID, as Rings authenticated it; FRAME a unibyte
string.")

(cl-defstruct (lispm-remote-node (:constructor lispm-remote-node--create)
                                 (:copier nil))
  "A running Rings node."
  (handle nil :documentation "The module's node.")
  (process nil :documentation "The pipe process the module wakes Emacs through.")
  (did nil :documentation "The node's DID.")
  (callbacks (make-hash-table :test #'eql)
             :documentation "Callbacks of sends and requests under way, by id.")
  (frame-function nil :documentation "Called with FROM and FRAME for each frame."))

(defvar lispm-remote-node--running nil
  "The nodes started and not stopped.")

;;;; The account

(defun lispm-remote-node-account-file ()
  "Return the file this Emacs's Rings account key is kept in."
  (expand-file-name "account" lispm-remote-directory))

(defun lispm-remote-node-account ()
  "Return this Emacs's Rings account key, making and keeping one the
first time, in a file only you can read."
  (let ((file (lispm-remote-node-account-file)))
    (if (file-readable-p file)
        (with-temp-buffer
          (insert-file-contents file)
          (string-trim (buffer-string)))
      (lispm-remote-module-load)
      (let ((account (lispm-remote-core-generate-account)))
        (with-file-modes #o700
          (make-directory lispm-remote-directory t))
        (with-file-modes #o600
          (with-temp-file file
            (insert account "\n")))
        account))))

;;;; Starting and stopping

(defun lispm-remote-node-store ()
  "Return the directory this Emacs's node keeps its replay records in."
  (let ((directory (expand-file-name "rings" lispm-remote-directory)))
    (with-file-modes #o700
      (make-directory directory t))
    directory))

(defun lispm-remote-node-start (&optional account frame-function store)
  "Start a Rings node and return it.
ACCOUNT is its Rings account key, by default this Emacs's own; see
`lispm-remote-node-account'.  FRAME-FUNCTION, when given, is called
with FROM and FRAME for each frame a peer sends to it.

STORE is the directory the node keeps its replay records in.  Rings
refuses a sender whose message sequences go back, so a node that
restarts with an account must find them again; this Emacs's own node
keeps them in `lispm-remote-node-store'.  Without STORE, another
account's records live in memory, for a node that never restarts."
  (lispm-remote-module-load)
  (let* ((store (or store (and (null account) (lispm-remote-node-store))))
         (account (or account (lispm-remote-node-account)))
         (node (lispm-remote-node--create :frame-function frame-function))
         (process (make-pipe-process
                   :name "lispm-remote-node" :noquery t :coding 'binary
                   :filter (lambda (_process _output) (lispm-remote-node--collect node)))))
    (condition-case error
        (setf (lispm-remote-node-handle node)
              (lispm-remote-core-start account lispm-remote-ice-servers process
                                       (and store (expand-file-name store))))
      (error (delete-process process)
             (signal (car error) (cdr error))))
    (setf (lispm-remote-node-process node) process
          (lispm-remote-node-did node) (lispm-remote-core-did (lispm-remote-node-handle node)))
    (push node lispm-remote-node--running)
    node))

(defun lispm-remote-node-live-p (node)
  "Return non-nil when NODE is running."
  (and (lispm-remote-node-handle node)
       (lispm-remote-core-live-p (lispm-remote-node-handle node))))

(defun lispm-remote-node-stop (node)
  "Stop NODE.  Sends and requests under way get no callback."
  (when-let* ((handle (lispm-remote-node-handle node)))
    ;; The module stops writing before its pipe goes: a write to a closed
    ;; pipe raises SIGPIPE, which ends a batch Emacs.
    (lispm-remote-core-stop handle))
  (when-let* ((process (lispm-remote-node-process node)))
    (delete-process process))
  (clrhash (lispm-remote-node-callbacks node))
  (setq lispm-remote-node--running (delq node lispm-remote-node--running)))

(defun lispm-remote-node--stop-all ()
  "Stop every running node, as Emacs exits."
  (mapc #'lispm-remote-node-stop lispm-remote-node--running))

(add-hook 'kill-emacs-hook #'lispm-remote-node--stop-all)

;;;; Work

(defun lispm-remote-node--expect (node id callback)
  "Remember CALLBACK for the outcome of NODE's work ID; return ID."
  (when callback
    (puthash id callback (lispm-remote-node-callbacks node)))
  id)

(defun lispm-remote-node-send (node to frame &optional callback)
  "Start sending FRAME, a unibyte string, from NODE to the peer TO, a DID.
CALLBACK, when given, is called with t and nil once the frame left, or
with nil and a message when it could not be sent."
  ;; `string-to-unibyte' refuses a character that is not a byte.
  (let ((frame (if (multibyte-string-p frame) (string-to-unibyte frame) frame)))
    (lispm-remote-node--expect
     node (lispm-remote-core-send (lispm-remote-node-handle node) to frame) callback)))

(defun lispm-remote-node-request (node method params &optional callback)
  "Start the Rings JSON-RPC request METHOD on NODE with PARAMS, an alist.
CALLBACK, when given, is called with the result, parsed into alists and
lists, and nil; or with nil and Rings's message when it failed."
  (let ((json (json-serialize (or params (make-hash-table)))))
    (lispm-remote-node--expect
     node (lispm-remote-core-request (lispm-remote-node-handle node) method json) callback)))

(defun lispm-remote-node-connect (node url &optional callback)
  "Join NODE to the network through the seed node at URL.
CALLBACK, when given, is called with the seed's DID and nil, or nil and
a message."
  (lispm-remote-node-request
   node "connectPeerViaHttp" `((url . ,url))
   (and callback
        (lambda (result error)
          (funcall callback (alist-get 'did result) error)))))

;;;; Outcomes

(defun lispm-remote-node--collect (node)
  "Take the events waiting in NODE and act on them."
  (when (lispm-remote-node-live-p node)
    (dolist (event (lispm-remote-core-events (lispm-remote-node-handle node)))
      (condition-case-unless-debug error
          (lispm-remote-node--dispatch node event)
        (error (message "LispM Remote: %s" (error-message-string error)))))))

(defun lispm-remote-node--finish (node id &rest arguments)
  "Call the callback of NODE's work ID, if any, with ARGUMENTS."
  (let ((callbacks (lispm-remote-node-callbacks node)))
    (when-let* ((callback (gethash id callbacks)))
      (remhash id callbacks)
      (apply callback arguments))))

(defun lispm-remote-node--dispatch (node event)
  "Act on EVENT, which NODE's module reported."
  (pcase event
    (`(frame ,from ,frame)
     (when-let* ((function (lispm-remote-node-frame-function node)))
       (funcall function from frame))
     (run-hook-with-args 'lispm-remote-node-frame-functions node from frame))
    (`(sent ,id) (lispm-remote-node--finish node id t nil))
    (`(reply ,id ,json)
     (lispm-remote-node--finish
      node id (json-parse-string json :object-type 'alist :array-type 'list
                                 :null-object nil :false-object nil)
      nil))
    (`(failed ,id ,message) (lispm-remote-node--finish node id nil message))
    (`(lost ,count)
     (display-warning 'lispm-remote
                      (format "%d frames were lost while Emacs was busy" count)))))

(provide 'lispm-remote-node)
;;; lispm-remote-node.el ends here
