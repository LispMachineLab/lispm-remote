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
;; page at lispm.ai/remote does.  LispM and the phone are each connected
;; to a third node only, which stands for the network.  The page itself,
;; its camera and a real network outage are the browser test's.
;;
;; The phone's commands act on LispM's agents.  With LispM checked out
;; beside this repository (../lisp_machine, or the directory of its Lisp in
;; $LISPM_LISP), they act on a real agent whose model is scripted;
;; without it, on a stand-in for the few functions LispM Remote calls.

;;; Code:

(require 'ert)
(require 'lispm-remote)
(require 'lispm-remote-display)
(require 'lispm-remote-node-test)

(defconst lispm-remote-test--lispm-lisp
  (let ((directory (or (getenv "LISPM_LISP")
                       (expand-file-name "../../lisp_machine/lisp"
                                         (file-name-directory
                                          (or load-file-name buffer-file-name))))))
    (and (file-readable-p (expand-file-name "lispm-runtime.el" directory))
         (file-name-as-directory directory)))
  "The directory of LispM's Lisp, when LispM is at hand.")

;; LispM's agents, without its interface; its directory is a scratch one
;; from the start, never the user's.
(when lispm-remote-test--lispm-lisp
  (defvar lispm-directory (make-temp-file "lispm-remote-test-lispm" t))
  (let ((directory lispm-directory))
    (add-hook 'kill-emacs-hook (lambda () (delete-directory directory t))))
  (add-to-list 'load-path lispm-remote-test--lispm-lisp)
  (require 'lispm-runtime)
  ;; Its conversations, as LispM Remote reads them; loaded now, so that
  ;; the variables it defines are special before the tests bind them.
  (require 'lispm-chat))

(declare-function lispm-remote-core-generate-account "lispm-remote-core")
(declare-function lispm-remote-core-generate-static "lispm-remote-core")
(declare-function lispm-remote-core-initiator "lispm-remote-core")
(declare-function lispm-remote-core-initiator-pair "lispm-remote-core")
(declare-function lispm-remote-core-initiator-connect "lispm-remote-core")
(declare-function lispm-remote-core-initiator-receive "lispm-remote-core")
(declare-function lispm-remote-core-initiator-send "lispm-remote-core")
(declare-function lispm-remote-core-initiator-tick "lispm-remote-core")
(declare-function lispm-remote-core-initiator-open-p "lispm-remote-core")
(declare-function lispm-remote-core-initiator-close "lispm-remote-core")

(cl-defstruct (lispm-remote-test-phone (:constructor lispm-remote-test--phone-create)
                                       (:copier nil))
  "A phone: a Rings node and the device's machine, with what it saw."
  account secret store node machine timer pairing fingerprint states failures
  unpaired send-errors asking asked-at displays reopen)

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
      (when (lispm-remote-test-phone-reopen phone)
        (setf (lispm-remote-test-phone-reopen phone) nil)
        (lispm-remote-test--phone-apply phone (lispm-remote-core-initiator-connect machine)))
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
       (if (= (aref payload 0) 1)
           ;; The display's messages: kept as they came, newest first.
           (push payload (lispm-remote-test-phone-displays phone))
         (setf (lispm-remote-test-phone-asking phone) nil)
         (push (json-parse-string (decode-coding-string payload 'utf-8)
                                  :object-type 'alist :array-type 'list)
               (lispm-remote-test-phone-states phone))))
      (`(failed . ,why)
       (push why (lispm-remote-test-phone-failures phone))
       ;; A session that could not open in time is tried again while the
       ;; phone is paired, as the page does: the network may have lost
       ;; every request of that try.
       (when (and (equal why '(timed-out))
                  (lispm-remote-test-phone-pairing phone))
         (setf (lispm-remote-test-phone-reopen phone) t))))))

(defun lispm-remote-test--last-state (phone)
  "Return the last state PHONE received."
  (alist-get 'state (seq-find (lambda (message) (equal (alist-get 'type message) "state"))
                              (lispm-remote-test-phone-states phone))))

(defun lispm-remote-test--agent-state (phone)
  "Return the state of the first agent in the last state PHONE received."
  (alist-get 'state (car (alist-get 'agents (lispm-remote-test--last-state phone)))))

(defun lispm-remote-test--agent (phone name)
  "Return the agent NAME in the last state PHONE received."
  (seq-find (lambda (agent) (equal (alist-get 'name agent) name))
            (alist-get 'agents (lispm-remote-test--last-state phone))))

(defvar lispm-remote-test--ids 0
  "The last id a test command was given.")

(defun lispm-remote-test--send (phone message)
  "Send MESSAGE, JSON as `json-serialize' takes it, from PHONE."
  (lispm-remote-test--phone-apply
   phone (lispm-remote-core-initiator-send (lispm-remote-test-phone-machine phone)
                                           (json-serialize message))))

(defun lispm-remote-test--command (phone message)
  "Send the command MESSAGE from PHONE until LispM answers, as the page
does, and return the answer."
  (let* ((id (format "c%d" (cl-incf lispm-remote-test--ids)))
         (message (cons (cons 'id id) message))
         (sent 0))
    (lispm-remote-node-test--wait
     (lambda ()
       (when (> (- (float-time) sent) 2)
         (setq sent (float-time))
         (lispm-remote-test--send phone message))
       (seq-find (lambda (answer) (equal (alist-get 'id answer) id))
                 (lispm-remote-test-phone-states phone)))
     (format "answer to %s" (alist-get 'type message)))))

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

(defun lispm-remote-test--pair (phone)
  "Pair PHONE, and wait for the state it sees first."
  (lispm-remote-approve (lispm-remote-test--claim phone))
  (lispm-remote-node-test--wait (lambda () (lispm-remote-test--last-state phone))
                                "state after pairing"))

;;;; LispM's agents

(defvar lispm-remote-test--responses nil
  "What LispM's scripted model answers, in order: messages, or `hang'.")

(defvar lispm-remote-test--requests nil
  "The requests LispM's scripted model received, newest first.")

(defvar lispm-remote-test--project nil
  "The project directory of the test's agent.")

(defvar lispm-remote-test--calls 0
  "The last call id the test's agent was given.")

(cl-defstruct (lispm-remote-test-agent (:constructor lispm-remote-test--agent-create)
                                       (:copier nil))
  "A stand-in for one of LispM's agents, when LispM is not at hand."
  name (state 'idle) told (calls (make-hash-table :test #'equal)) decided allowed
  items (buffer (generate-new-buffer " *stand-in session*")))

(defvar lispm-remote-test--answer nil
  "What the test's agent answers next, when it is told something.")

(defvar lispm-remote-test--stand-in nil
  "The stand-in agent, when LispM is not at hand.")

(defun lispm-remote-test--fake-api (_backend body _betas &rest callbacks)
  "Answer BODY with the next of `lispm-remote-test--responses', through
CALLBACKS, as LispM's backends do."
  (push body lispm-remote-test--requests)
  (let ((response (pop lispm-remote-test--responses)))
    (unless (eq response 'hang)
      (run-at-time 0.01 nil
                   (lambda ()
                     (if response
                         (funcall (plist-get callbacks :on-done) response)
                       (funcall (plist-get callbacks :on-error)
                                '(:status 400 :type "test_error" :message "no more answers"))))))
    nil))

(defun lispm-remote-test--reply (stop &rest blocks)
  "A message of the model with BLOCKS, which stopped for STOP."
  `((role . "assistant") (content . ,(vconcat blocks)) (stop_reason . ,stop)
    (usage (input_tokens . 10) (output_tokens . 5))))

(defun lispm-remote-test--real-agent ()
  "The test's agent in LispM."
  (lispm-get-agent "tester"))

(defun lispm-remote-test--stand-in-changed ()
  "Tell LispM Remote that the stand-in agent changed, as LispM does."
  (run-hook-with-args 'lispm-state-change-functions lispm-remote-test--stand-in))

(defmacro lispm-remote-test--with-lispm (&rest body)
  "Run BODY with one agent of LispM, called tester, idle in a scratch
project: a real one when LispM is at hand, else a stand-in."
  (declare (indent 0))
  `(if lispm-remote-test--lispm-lisp
       (lispm-remote-test--with-real-lispm (lambda () ,@body))
     (lispm-remote-test--with-stand-in (lambda () ,@body))))

(defun lispm-remote-test--with-real-lispm (body)
  "Call BODY with a real agent of LispM, in scratch directories, its model
scripted."
  (let* ((tmp (file-name-as-directory (make-temp-file "lispm-remote-lispm" t)))
         (lispm-directory (expand-file-name "home/" tmp))
         (lispm-remote-test--project (expand-file-name "project/" tmp))
         (lispm-git nil)
         (lispm-auto-title nil)
         (lispm-backends (cons '(test :api anthropic :url "http://fake.invalid" :model "test-model")
                               lispm-backends))
         (lispm-backend 'test)
         (lispm--agents nil)
         (lispm--roles (copy-sequence lispm--roles))
         (lispm-api-function #'lispm-remote-test--fake-api)
         (lispm-remote-test--responses nil)
         (lispm-remote-test--requests nil)
         (lispm--idle-work (make-hash-table :test #'equal))
         (process-environment (lispm--git-environment)))
    (make-directory lispm-remote-test--project t)
    (unwind-protect
        (progn
          (eval `(lispm-defagent tester :root ,lispm-remote-test--project) t)
          (funcall body))
      (dolist (agent (lispm-agents))
        (lispm-unwatch agent))
      (maphash (lambda (_ timer) (cancel-timer timer)) lispm--idle-work)
      (dolist (buffer (buffer-list))
        (when (and (buffer-file-name buffer) (file-in-directory-p (buffer-file-name buffer) tmp))
          (with-current-buffer buffer (set-buffer-modified-p nil))
          (kill-buffer buffer)))
      (delete-directory tmp t))))

(defun lispm-remote-test--with-stand-in (body)
  "Call BODY with LispM's few functions that LispM Remote calls standing in
for LispM, over one agent."
  (let* ((agent (lispm-remote-test--agent-create :name 'tester))
         (lispm-remote-test--stand-in agent)
         (lispm-remote-test--project "/tmp/project/")
         (waiting (lambda (agent)
                    (let (ids)
                      (maphash (lambda (id _) (push id ids)) (lispm-remote-test-agent-calls agent))
                      (sort ids #'string<))))
         (settle (lambda (agent id decision)
                   (push (cons id decision) (lispm-remote-test-agent-decided agent))
                   (remhash id (lispm-remote-test-agent-calls agent))
                   (when (zerop (hash-table-count (lispm-remote-test-agent-calls agent)))
                     (setf (lispm-remote-test-agent-state agent) 'idle))
                   (lispm-remote-test--stand-in-changed))))
    (cl-letf (((symbol-function 'lispm-agents) (lambda () (list agent)))
              ((symbol-function 'lispm-get-agent)
               (lambda (name &optional _noerror) (and (equal name "tester") agent)))
              ((symbol-function 'lispm-agent-name) #'lispm-remote-test-agent-name)
              ((symbol-function 'lispm-agent-role) (lambda (_) 'assistant))
              ((symbol-function 'lispm-agent-root) (lambda (_) lispm-remote-test--project))
              ((symbol-function 'lispm-agent-state) #'lispm-remote-test-agent-state)
              ((symbol-function 'lispm-agent-status) (lambda (_) nil))
              ((symbol-function 'lispm-agent-pending) #'lispm-remote-test-agent-calls)
              ((symbol-function 'lispm-waiting-calls) waiting)
              ((symbol-function 'lispm-asked) #'identity)
              ((symbol-function 'lispm-agent-buffer) #'lispm-remote-test-agent-buffer)
              ((symbol-function 'lispm-chat-items)
               (lambda (_buffer) (reverse (lispm-remote-test-agent-items agent))))
              ((symbol-function 'lispm-tell)
               (lambda (agent text &rest _)
                 (push text (lispm-remote-test-agent-told agent))
                 (push (list :kind 'user :text text) (lispm-remote-test-agent-items agent))
                 (when lispm-remote-test--answer
                   (push (list :kind 'text :text lispm-remote-test--answer)
                         (lispm-remote-test-agent-items agent)))
                 (lispm-remote-test--stand-in-changed)
                 agent))
              ((symbol-function 'lispm-approve-for)
               (lambda (agent &optional id) (funcall settle agent id 'approve)))
              ((symbol-function 'lispm-allow)
               (lambda (agent id)
                 (setf (lispm-remote-test-agent-allowed agent) t)
                 (funcall settle agent id 'approve)))
              ((symbol-function 'lispm-deny-for)
               (lambda (agent _reason &optional id) (funcall settle agent id 'deny)))
              ((symbol-function 'lispm-abort)
               (lambda (agent)
                 (when (eq (lispm-remote-test-agent-state agent) 'idle)
                   (user-error "%s is idle" (lispm-remote-test-agent-name agent)))
                 (setf (lispm-remote-test-agent-state agent) 'idle)
                 (lispm-remote-test--stand-in-changed))))
      (unwind-protect
          (funcall body)
        (kill-buffer (lispm-remote-test-agent-buffer agent))))))

(defun lispm-remote-test--answer-next (text)
  "Have the agent answer TEXT when it is told something next."
  (if lispm-remote-test--lispm-lisp
      (setq lispm-remote-test--responses
            (list (lispm-remote-test--reply "end_turn" `((type . "text") (text . ,text)))))
    (setq lispm-remote-test--answer text)))

(defun lispm-remote-test--ask ()
  "Have the agent ask to write a file in its project; return the call's id
and the file."
  (let* ((id (format "t%d" (cl-incf lispm-remote-test--calls)))
         (file (format "note-%s.txt" id)))
    (if lispm-remote-test--lispm-lisp
        (let ((agent (lispm-remote-test--real-agent)))
          (setq lispm-remote-test--responses
                (list (lispm-remote-test--reply
                       "tool_use" `((type . "tool_use") (id . ,id) (name . "write_file")
                                    (input (path . ,file) (content . "hello\n"))))
                      (lispm-remote-test--reply "end_turn" '((type . "text") (text . "done")))))
          (lispm-tell agent (format "write %s" file))
          (lispm-remote-node-test--wait (lambda () (member id (lispm-waiting-calls agent)))
                                        "a call awaiting approval"))
      (let ((agent lispm-remote-test--stand-in))
        (puthash id (list :name "write_file" :input `((path . ,file) (content . "hello\n")))
                 (lispm-remote-test-agent-calls agent))
        (setf (lispm-remote-test-agent-state agent) 'waiting)
        (lispm-remote-test--stand-in-changed)))
    (cons id file)))

(defun lispm-remote-test--decided (call)
  "Return how the agent's CALL was settled: `approve', `deny', or nil."
  (if lispm-remote-test--lispm-lisp
      (let ((file (expand-file-name (cdr call) lispm-remote-test--project))
            (agent (lispm-remote-test--real-agent)))
        (cond ((member (car call) (lispm-waiting-calls agent)) nil)
              ((not (eq (lispm-agent-state agent) 'idle)) nil)
              ((file-exists-p file) 'approve)
              (t 'deny)))
    (alist-get (car call) (lispm-remote-test-agent-decided lispm-remote-test--stand-in)
               nil nil #'equal)))

(defun lispm-remote-test--allowed-p ()
  "Return non-nil when the agent may make calls like the one approved for
good without asking."
  (if lispm-remote-test--lispm-lisp
      (lispm-agent-allowed (lispm-remote-test--real-agent))
    (lispm-remote-test-agent-allowed lispm-remote-test--stand-in)))

(defun lispm-remote-test--told ()
  "Return the texts the agent was told: with LispM, those of the user's
messages in its latest request, which holds the whole session."
  (if lispm-remote-test--lispm-lisp
      (let (told)
        (seq-doseq (message (alist-get 'messages (car lispm-remote-test--requests)))
          (when (equal (alist-get 'role message) "user")
            (let ((content (alist-get 'content message)))
              (if (stringp content)
                  (push content told)
                (seq-doseq (block content)
                  (when (equal (alist-get 'type block) "text")
                    (push (alist-get 'text block) told)))))))
        told)
    (lispm-remote-test-agent-told lispm-remote-test--stand-in)))

(defun lispm-remote-test--told-count (text)
  "Return how often the agent was told TEXT."
  (seq-count (lambda (told) (string-search text told)) (lispm-remote-test--told)))

(defun lispm-remote-test--make-busy ()
  "Have the agent start a turn that does not end by itself."
  (if lispm-remote-test--lispm-lisp
      (let ((agent (lispm-remote-test--real-agent)))
        (setq lispm-remote-test--responses '(hang))
        (lispm-tell agent "think forever")
        (lispm-remote-node-test--wait (lambda () (eq (lispm-agent-state agent) 'requesting))
                                      "a turn"))
    (setf (lispm-remote-test-agent-state lispm-remote-test--stand-in) 'requesting)
    (lispm-remote-test--stand-in-changed)))

(defun lispm-remote-test--idle-p ()
  "Return non-nil when the agent is idle, with nothing more to do."
  (if lispm-remote-test--lispm-lisp
      (let ((agent (lispm-remote-test--real-agent)))
        (and (eq (lispm-agent-state agent) 'idle) (null (lispm-agent-inbox agent))))
    (eq (lispm-remote-test-agent-state lispm-remote-test--stand-in) 'idle)))

(defmacro lispm-remote-test--with-agents (bindings &rest body)
  "Run BODY with LispM Remote on, a phone paired with it, and LispM's agent
tester, bound as BINDINGS says: (PHONE)."
  (declare (indent 1))
  `(lispm-remote-test--with-world (_relay ,(car bindings))
     (lispm-remote-test--with-lispm
       (let ((lispm-remote-state-function #'lispm-remote-default-state))
         (lispm-remote-test--pair ,(car bindings))
         ,@body))))

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
      (should (string-search "Paired with Test Phone" (buffer-string)))
      ;; The phone opened its session: every step is done.
      (should (string-search "Session open" (buffer-string)))
      (should (string-search "✔ Connected" (buffer-string))))
    (should-not (lispm-remote--waiting-p lispm-remote--offer))
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
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-pairing phone))
                                  "pairing on the phone")
    (lispm-remote-forget (lispm-remote-node-did (lispm-remote-test-phone-node phone)))
    (should-not (lispm-remote--devices))
    ;; The phone comes back and is told it is no longer paired.
    (lispm-remote-test--phone-stop phone)
    (lispm-remote-node-test--offline 8)
    (lispm-remote-test--phone-start phone)
    (lispm-remote-node-test--connect (lispm-remote-test-phone-node phone) relay)
    (lispm-remote-test--phone-apply
     phone (lispm-remote-core-initiator-connect (lispm-remote-test-phone-machine phone)))
    (ert-info ((lambda ()
                 (format "phone failures: %S\nphone send errors: %S\nLispM log:\n%s"
                         (lispm-remote-test-phone-failures phone)
                         (lispm-remote-test-phone-send-errors phone)
                         (with-current-buffer (get-buffer-create lispm-remote--log-buffer)
                           (buffer-string)))))
      (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-unpaired phone))
                                    "refusal of the forgotten phone"))
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

;;;; Commands

(defun lispm-remote-test--approvals (phone)
  "Return the approvals of the agent tester in PHONE's last state."
  (alist-get 'approvals (lispm-remote-test--agent phone "tester")))

(defun lispm-remote-test--decide (phone call decision)
  "Have PHONE answer the agent's CALL by DECISION; return LispM's answer."
  (lispm-remote-node-test--wait
   (lambda () (seq-find (lambda (approval) (equal (alist-get 'call approval) (car call)))
                        (lispm-remote-test--approvals phone)))
   "the approval on the phone")
  (lispm-remote-test--command
   phone `((type . "decide") (agent . "tester") (call . ,(car call)) (decision . ,decision))))

;;;; The pairing screen

(defun lispm-remote-test--steps (offer &optional peers)
  "Return the step states of OFFER with PEERS peers, as a list."
  (let ((lispm-remote--peers peers))
    (mapcar #'cdr (lispm-remote--progress offer))))

(ert-deftest lispm-remote-test-the-steps-follow-pairing ()
  (let ((code '(:token "t")))
    (should (equal (lispm-remote-test--steps code) '(active active pending pending pending)))
    (should (equal (lispm-remote-test--steps code 0) '(active active pending pending pending)))
    (should (equal (lispm-remote-test--steps code 2) '(done active pending pending pending)))
    (should (equal (lispm-remote-test--steps (append '(:fingerprint "F" :name "N") code))
                   '(done done done active pending)))
    (should (equal (lispm-remote-test--steps (append '(:paired "N") code))
                   '(done done done done active)))
    (should (equal (lispm-remote-test--steps (append '(:paired "N" :opened t) code))
                   '(done done done done done)))
    ;; A code that ends fails the step it waited at.
    (should (equal (lispm-remote-test--steps (append '(:ended expired) code) 1)
                   '(done failed pending pending pending)))
    (should (equal (lispm-remote-test--steps (append '(:ended refused :fingerprint "F") code))
                   '(done done done failed pending)))))

(ert-deftest lispm-remote-test-the-countdown-bar-keeps-its-width ()
  (dolist (fraction '(0 0.01 0.13 0.5 0.99 1 1.5 -1))
    (should (= (length (lispm-remote--bar-text fraction 30)) 30)))
  (should (string-prefix-p "███" (lispm-remote--bar-text 0.5 30)))
  (should (string-suffix-p "─" (lispm-remote--bar-text 0.5 30))))

(ert-deftest lispm-remote-test-the-screen-reads-the-same-in-a-terminal ()
  (skip-unless (lispm-remote-node-test--module-p))
  (let ((lispm-remote--offer (list :token "AQID" :expires (+ (float-time) 90)))
        (lispm-remote--view 'pair)
        (lispm-remote--peers 1)
        (lispm-remote-offer-seconds 120))
    (unwind-protect
        (with-current-buffer (lispm-remote--render)
          (let ((text (buffer-string)))
            (should (string-search "✔ Join ── ◉ Scan ── ○ Reach" text))
            (should (string-search "scan this code" text))
            (should (string-match-p "1:[23][0-9] left" text))
            (should (string-search "On Rings, 1 peer" text)))
          ;; The screen redraws itself while it waits, and stops when
          ;; the buffer goes.
          (should lispm-remote--ticker)
          (let ((ticker lispm-remote--ticker))
            (kill-buffer)
            (should-not (memq ticker timer-list))))
      (when (get-buffer lispm-remote--buffer)
        (kill-buffer lispm-remote--buffer)))))

(ert-deftest lispm-remote-test-the-screen-stops-ticking-once-paired ()
  (let ((lispm-remote--offer (list :token "AQID" :expires (+ (float-time) 90)
                                   :fingerprint "4F2A-91C0" :name "Phone"))
        (lispm-remote--view 'pair))
    (unwind-protect
        (with-current-buffer (lispm-remote--render)
          (should (string-search "4F2A-91C0" (buffer-string)))
          (should (string-search "Approve (a)" (buffer-string)))
          (should lispm-remote--ticker)
          (setq lispm-remote--offer (append '(:paired "Phone" :opened t) lispm-remote--offer))
          (lispm-remote--render)
          (should-not lispm-remote--ticker)
          (should (string-search "Paired with Phone" (buffer-string))))
      (kill-buffer lispm-remote--buffer))))

;;;; Home

(ert-deftest lispm-remote-test-home-says-it-is-off-and-lists-no-device ()
  (let ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
        (lispm-remote--view 'home))
    (unwind-protect
        (with-current-buffer (lispm-remote--render)
          (should (derived-mode-p 'lispm-remote-home-mode))
          (should (string-search "○ Off: no device can reach LispM" (buffer-string)))
          (should (string-search "Turn on (t)" (buffer-string)))
          (should (string-search "None yet." (buffer-string)))
          (should (string-search "Pair a device (p)" (buffer-string)))
          (should-not lispm-remote--ticker))
      (kill-buffer lispm-remote--buffer)
      (delete-directory lispm-remote-directory t))))

(ert-deftest lispm-remote-test-a-device-paired-before-rings-0.37-is-told-to-pair-again ()
  (let ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
        (lispm-remote--view 'home)
        (old '(:peer "0x1f0f8a4b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f" :key "k" :name "Old Phone"
                     :paired-at 1792700000000)))
    (unwind-protect
        (progn
          (lispm-remote--save-devices (list old))
          (with-current-buffer (lispm-remote--render)
            (should (string-search "Old Phone" (buffer-string)))
            (should (string-search lispm-remote--moved-device-text (buffer-string))))
          ;; The phone pairs again at its new address with the same key: the
          ;; machine stores the new record and forgets the old one.
          (lispm-remote--apply
           `((paired "0x3bbba89641b3470b86f516a9e91862ce6b23765b" "k" "Old Phone" 1792800000000)
             (forgotten ,(plist-get old :peer))))
          (let ((devices (lispm-remote--devices)))
            (should (= (length devices) 1))
            (should (equal (plist-get (car devices) :rings) lispm-remote--rings)))
          (setq lispm-remote--view 'home)
          (with-current-buffer (lispm-remote--render)
            (should (string-search "Old Phone" (buffer-string)))
            (should-not (string-search lispm-remote--moved-device-text (buffer-string)))))
      (when (buffer-live-p (get-buffer lispm-remote--buffer))
        (kill-buffer lispm-remote--buffer))
      (setq lispm-remote--offer nil)
      (delete-directory lispm-remote-directory t))))

(ert-deftest lispm-remote-test-home-follows-pairing-and-sessions ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay phone)
    (lispm-remote)
    (with-current-buffer lispm-remote--buffer
      (should (derived-mode-p 'lispm-remote-home-mode))
      (should (string-search "Turn off (t)" (buffer-string)))
      (should (string-search (lispm-remote-node-did lispm-remote--node) (buffer-string))))
    ;; Pairing takes the window over, and q brings home back.
    (let ((fingerprint (lispm-remote-test--claim phone)))
      (with-current-buffer lispm-remote--buffer
        (should (derived-mode-p 'lispm-remote-pair-mode)))
      (lispm-remote-approve fingerprint))
    (lispm-remote-node-test--wait (lambda () (lispm-remote-test-phone-states phone))
                                  "state after pairing")
    (lispm-remote-withdraw)
    (with-current-buffer lispm-remote--buffer
      (should (derived-mode-p 'lispm-remote-home-mode))
      (should (string-search "Test Phone" (buffer-string)))
      (should (string-search "● connected" (buffer-string))))
    ;; The phone goes; home says so once its session closes.
    (lispm-remote-test--phone-apply
     phone (lispm-remote-core-initiator-close (lispm-remote-test-phone-machine phone)))
    (lispm-remote-node-test--wait
     (lambda () (with-current-buffer lispm-remote--buffer
                  (string-search "○ not connected" (buffer-string))))
     "home to see the phone go")))

(ert-deftest lispm-remote-test-a-phone-sees-approvals-and-answers-them ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-agents (phone)
    (should (equal (lispm-remote-test--approvals phone) nil))
    ;; The phone sees what the agent waits on, and allows it.
    (let ((call (lispm-remote-test--ask)))
      (lispm-remote-node-test--wait (lambda () (lispm-remote-test--approvals phone))
                                    "approvals in the state")
      (let ((approval (car (lispm-remote-test--approvals phone))))
        (should (equal (alist-get 'call approval) (car call)))
        (should (equal (alist-get 'tool approval) "write_file"))
        (should (equal (alist-get 'summary approval) (cdr call))))
      (should (equal (alist-get 'type (lispm-remote-test--decide phone call "allow")) "ack"))
      (should (eq (lispm-remote-node-test--wait (lambda () (lispm-remote-test--decided call))
                                                "the call settled")
                  'approve))
      (should-not (lispm-remote-test--allowed-p))
      (lispm-remote-node-test--wait (lambda () (null (lispm-remote-test--approvals phone)))
                                    "the approval gone from the phone"))
    ;; A decision that is none changes nothing; the phone then denies.
    (let ((call (lispm-remote-test--ask)))
      (let ((answer (lispm-remote-test--decide phone call "maybe")))
        (should (equal (alist-get 'type answer) "error"))
        (should (equal (alist-get 'command answer) "decide"))
        (should (string-search "allow, always or deny" (alist-get 'message answer))))
      (should-not (lispm-remote-test--decided call))
      (should (equal (alist-get 'type (lispm-remote-test--decide phone call "deny")) "ack"))
      (should (eq (lispm-remote-node-test--wait (lambda () (lispm-remote-test--decided call))
                                                "the call settled")
                  'deny)))
    ;; A call the agent does not wait on is an error.
    (let ((answer (lispm-remote-test--command
                   phone '((type . "decide") (agent . "tester") (call . "t99") (decision . "allow")))))
      (should (equal (alist-get 'type answer) "error"))
      (should (string-search "waits on no call t99" (alist-get 'message answer))))
    ;; Last, as it allows the calls after it: one allowed for good.
    (let ((call (lispm-remote-test--ask)))
      (should (equal (alist-get 'type (lispm-remote-test--decide phone call "always")) "ack"))
      (should (eq (lispm-remote-node-test--wait (lambda () (lispm-remote-test--decided call))
                                                "the call settled")
                  'approve))
      (should (lispm-remote-test--allowed-p)))))

(ert-deftest lispm-remote-test-a-phone-talks-to-an-agent-and-stops-it ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-agents (phone)
    (when lispm-remote-test--lispm-lisp
      (setq lispm-remote-test--responses
            (list (lispm-remote-test--reply "end_turn" '((type . "text") (text . "hi"))))))
    (should (equal (alist-get 'type (lispm-remote-test--command
                                     phone '((type . "say") (agent . "tester") (text . "hello"))))
                   "ack"))
    (lispm-remote-node-test--wait (lambda () (= (lispm-remote-test--told-count "hello") 1))
                                  "the agent told")
    (lispm-remote-node-test--wait #'lispm-remote-test--idle-p "the turn's end")
    ;; Nobody to tell, or nothing to say.
    (let ((answer (lispm-remote-test--command
                   phone '((type . "say") (agent . "nobody") (text . "hello")))))
      (should (equal (alist-get 'type answer) "error"))
      (should (string-search "No agent is called nobody" (alist-get 'message answer))))
    (should (equal (alist-get 'type (lispm-remote-test--command
                                     phone '((type . "say") (agent . "tester") (text . "  "))))
                   "error"))
    ;; A turn under way, as the phone sees it, stopped from the phone.
    (lispm-remote-test--make-busy)
    (lispm-remote-node-test--wait
     (lambda () (equal (alist-get 'state (lispm-remote-test--agent phone "tester")) "requesting"))
     "the turn on the phone")
    (should (equal (alist-get 'type (lispm-remote-test--command
                                     phone '((type . "interrupt") (agent . "tester"))))
                   "ack"))
    (should (lispm-remote-test--idle-p))
    (lispm-remote-node-test--wait
     (lambda () (equal (alist-get 'state (lispm-remote-test--agent phone "tester")) "idle"))
     "the agent idle on the phone")
    ;; An idle agent has no turn to stop.
    (let ((answer (lispm-remote-test--command phone '((type . "interrupt") (agent . "tester")))))
      (should (equal (alist-get 'type answer) "error"))
      (should (string-search "idle" (alist-get 'message answer))))))

(ert-deftest lispm-remote-test-a-repeated-command-is-done-once ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-agents (phone)
    (when lispm-remote-test--lispm-lisp
      (setq lispm-remote-test--responses
            (list (lispm-remote-test--reply "end_turn" '((type . "text") (text . "hi"))))))
    (let ((command '((id . "once") (type . "say") (agent . "tester") (text . "only once"))))
      (dotimes (_ 3)
        (lispm-remote-test--send phone command))
      (lispm-remote-node-test--wait
       (lambda () (= (seq-count (lambda (answer) (equal (alist-get 'id answer) "once"))
                                (lispm-remote-test-phone-states phone))
                     3))
       "three answers")
      (should (seq-every-p (lambda (answer)
                             (or (not (equal (alist-get 'id answer) "once"))
                                 (equal (alist-get 'type answer) "ack")))
                           (lispm-remote-test-phone-states phone)))
      (lispm-remote-node-test--wait #'lispm-remote-test--idle-p "the turn's end")
      (should (= (lispm-remote-test--told-count "only once") 1)))))

(ert-deftest lispm-remote-test-commands-need-lispm ()
  (cl-letf (((symbol-function 'lispm-get-agent) nil))
    (let ((answer (lispm-remote--do "peer" "channel" "say" '((id . 7) (agent . "coder") (text . "hello")))))
      (should (equal answer '((type . "error") (command . "say") (id . 7)
                              (message . "LispM is not running in this Emacs")))))))

(ert-deftest lispm-remote-test-a-call-is-summed-up-briefly ()
  (should (equal (lispm-remote--summary '((command . "make test") (why . "check"))) "make test"))
  (should (equal (lispm-remote--summary '((path . "a.el") (content . "x"))) "a.el"))
  (should (equal (lispm-remote--summary '((n . 3))) "{\"n\":3}"))
  (should (equal (lispm-remote--summary nil) ""))
  (let ((summary (lispm-remote--summary `((code . ,(make-string 1000 ?x))))))
    (should (= (length summary) lispm-remote--summary-length))
    (should (string-suffix-p "…" summary))))

;;;; Conversations

(defun lispm-remote-test--chat (phone)
  "Return the last chat PHONE received."
  (seq-find (lambda (message) (equal (alist-get 'type message) "chat"))
            (lispm-remote-test-phone-states phone)))

(defun lispm-remote-test--said (phone kind)
  "Return the texts of the items of KIND in the last chat PHONE received."
  (mapcar (lambda (item) (alist-get 'text item))
          (seq-filter (lambda (item) (equal (alist-get 'kind item) kind))
                      (alist-get 'items (lispm-remote-test--chat phone)))))

(ert-deftest lispm-remote-test-a-phone-follows-an-agents-conversation ()
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-agents (phone)
    (let ((hello (concat "Hello " (string #x3bb #x4f60 #x597d) " ✓"))
          (lispm-remote-test--answer nil))
      (should (equal (alist-get 'type (lispm-remote-test--command
                                       phone '((type . "watch") (agent . "tester"))))
                     "ack"))
      (lispm-remote-node-test--wait (lambda () (lispm-remote-test--chat phone)) "the conversation")
      (should (equal (alist-get 'agent (lispm-remote-test--chat phone)) "tester"))
      ;; What the phone says, and the answer, show in the conversation,
      ;; whatever the script.
      (lispm-remote-test--answer-next "Hi, phone.")
      (lispm-remote-test--command phone `((type . "say") (agent . "tester") (text . ,hello)))
      (lispm-remote-node-test--wait
       (lambda () (and (member hello (lispm-remote-test--said phone "user"))
                       (member "Hi, phone." (lispm-remote-test--said phone "text"))))
       "the answer in the conversation")
      (should (= (alist-get 'more (lispm-remote-test--chat phone)) 0))
      ;; Nobody to follow.
      (let ((answer (lispm-remote-test--command
                     phone '((type . "watch") (agent . "nobody")))))
        (should (equal (alist-get 'type answer) "error"))
        (should (string-search "No agent is called nobody" (alist-get 'message answer))))
      ;; Following none.
      (should (equal (alist-get 'type (lispm-remote-test--command phone '((type . "watch"))))
                     "ack"))
      (should-not (lispm-remote--session-watch (car lispm-remote--sessions))))))

(ert-deftest lispm-remote-test-a-conversation-is-cut-to-fit-one-record ()
  (let* ((buffer (generate-new-buffer " *session*"))
         (items (cl-loop for i below 100
                         collect (list :kind (if (cl-oddp i) 'text 'user)
                                       :text (make-string (if (= i 99) 9000 1500) ?x))))
         (call (list :kind 'tool :name "run_shell" :state "DONE"
                     :input '((command . "make test")) :result (make-string 2000 ?y))))
    (unwind-protect
        (cl-letf (((symbol-function 'lispm-get-agent)
                   (lambda (name &optional _noerror) (and (equal name "tester") 'tester)))
                  ((symbol-function 'lispm-agent-buffer) (lambda (_) buffer))
                  ((symbol-function 'lispm-chat-items) (lambda (_) (append items (list call)))))
          (let* ((chat (lispm-remote--chat "tester"))
                 (kept (alist-get 'items chat))
                 (last (aref kept (1- (length kept))))
                 (before (aref kept (- (length kept) 2))))
            (should (<= (length kept) lispm-remote-chat-items))
            (should (= (+ (length kept) (alist-get 'more chat)) 101))
            (should (<= (string-bytes (json-serialize kept)) lispm-remote--chat-bytes))
            ;; Newest last; long texts and results cut short.
            (should (equal (alist-get 'kind last) "tool"))
            (should (equal (alist-get 'summary last) "make test"))
            (should (= (length (alist-get 'result last)) lispm-remote--chat-result-length))
            (should (= (length (alist-get 'text before)) lispm-remote--chat-text-length))
            (should (string-suffix-p "…" (alist-get 'text before))))
          (should (equal (lispm-remote--chat "nobody")
                         '((type . "chat") (agent . "nobody") (items . []) (more . 0)))))
      (kill-buffer buffer))))

;;;; LispM's windows over the channel

(defun lispm-remote-test--display-received (phone tag)
  "Return the display messages tagged TAG that PHONE received."
  (seq-filter (lambda (payload) (= (aref payload 1) tag))
              (lispm-remote-test-phone-displays phone)))

(defun lispm-remote-test--display-send (phone payload)
  "Send the display message PAYLOAD from PHONE."
  (lispm-remote-test--phone-apply
   phone (lispm-remote-core-initiator-send (lispm-remote-test-phone-machine phone) payload)))

(ert-deftest lispm-remote-test-a-phone-opens-lispms-windows-over-its-channel ()
  "A paired phone says hello to the display over its channel, sees the
windows it may open, opens one, and is sent that window's screen; LispM
made a tty frame for it, and only after the phone asked."
  (skip-unless (lispm-remote-node-test--module-p))
  (lispm-remote-test--with-world (_relay phone)
    (unwind-protect
        (progn
          (lispm-remote-test--pair phone)
          (should-not lispm-remote-display--channels)
          ;; Hello: version 1, tag 8, input numbered from 1.
          (lispm-remote-test--display-send phone (unibyte-string 1 8 1))
          (lispm-remote-node-test--wait
           (lambda () (lispm-remote-test--display-received phone 1)) "the windows")
          ;; Open "notes" at 40x10, cells 8x16: version 1, tag 1.
          (with-current-buffer (get-buffer-create "notes")
            (insert "Typed in LispM, redrawn on the phone."))
          (lispm-remote-test--display-send
           phone (concat (unibyte-string 1 1 5) "notes" (unibyte-string 40 10 8 16)))
          (lispm-remote-node-test--wait
           (lambda () (lispm-remote-test--display-received phone 3)) "a frame")
          (should (lispm-remote-test--display-received phone 2))
          (let ((surface (caar lispm-remote-display--frames)))
            (should surface)
            ;; A batch Emacs redraws only when asked.
            (lispm-remote-node-test--wait
             (lambda ()
               (redisplay t)
               (string-search "Typed in LispM, redrawn on the phone."
                              (or (lispm-remote-core-display-text lispm-remote-display--handle
                                                                  surface)
                                  "")))
             "the window drawn")))
      (lispm-remote-display-stop)
      (when (get-buffer "notes")
        (kill-buffer "notes")))))

;;;; Versions

(defconst lispm-remote-test--mismatch
  "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Answer offer info failed: Codec deserialization error..."
  "Why LispM Remote 0.1.0, on Rings 0.34.2, could not join through
node.rings.rs once it ran Rings 0.36.0, word for word.  The module's
test `a_seed_of_another_rings_version_cannot_read_the_offer' holds Rings
to the words that tell it.")

(defconst lispm-remote-test--refused
  "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning..."
  "Why this LispM Remote cannot join through a seed whose handshake
carries a wire version (RingsNetwork/rings#1106), word for word.  The
seed's JSON-RPC code and data do not come through; the module's test
`a_seed_with_a_wire_version_refuses_this_node_in_words' holds Rings to
these words, from a stand-in seed.")

(defconst lispm-remote-test--phone-did "0x2222222222222222222222222222222222222222"
  "The DID of a device in the tests of versions.")

(defmacro lispm-remote-test--with-home (&rest body)
  "Run BODY with LispM Remote's home on show, as if `lispm-remote-mode'
were on, off the network: no node, no seed joined."
  (declare (indent 0))
  `(let ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
         (lispm-remote-mode t)
         (lispm-remote--node (lispm-remote-node--create :did "0xemacs"))
         (lispm-remote--view 'home)
         (lispm-remote--peers 0)
         (lispm-remote--rings "0.36.0")
         (lispm-remote--mismatches nil)
         (lispm-remote--peer-versions nil)
         (inhibit-message t))
     (unwind-protect
         (progn ,@body)
       (when (get-buffer lispm-remote--buffer)
         (kill-buffer lispm-remote--buffer))
       (delete-directory lispm-remote-directory t))))

(defun lispm-remote-test--home-text ()
  "Return what LispM Remote's home says now."
  (with-current-buffer (lispm-remote--render)
    (buffer-string)))

(ert-deftest lispm-remote-test-a-seed-of-another-rings-is-told-by-its-words ()
  (should (lispm-remote-rings-mismatch-p lispm-remote-test--mismatch))
  (should (lispm-remote-rings-mismatch-p lispm-remote-test--refused))
  ;; This node reading a seed's answer it cannot decode.
  (should (lispm-remote-rings-mismatch-p "Internal rpc services error: Server error: Decode error.."))
  (dolist (other '("Internal rpc services error: Server error: a handshake with 0x50e92c90d7a57bd01f4184eda98f87c382da4e61 is already in flight.."
                   "Internal rpc services error: Server error: Found existing transport.."
                   "Connect remote rpc server failed: HTTP status client error (429 Too Many Requests)"
                   "error sending request for url (https://node.rings.rs/)"
                   ""))
    (should-not (lispm-remote-rings-mismatch-p other)))
  (should-not (lispm-remote-rings-mismatch-p nil))
  (should (equal (lispm-remote--version-number "0.36.0-99feaaf") "0.36.0"))
  (should (equal (lispm-remote--version-number "0.36.0") "0.36.0"))
  (should-not (lispm-remote--version-number "unknown")))

(ert-deftest lispm-remote-test-a-seed-of-another-rings-is-named-and-left-alone ()
  "One failed join says what is wrong, on the window and in the Join
step, and LispM Remote stops joining through that seed for a while."
  (lispm-remote-test--with-home
    (let ((lispm-remote-seeds '("https://seed.example"))
          (joins 0)
          (answer lispm-remote-test--mismatch))
      (cl-letf (((symbol-function 'lispm-remote-node-connect)
                 (lambda (_node _seed callback)
                   (cl-incf joins)
                   (funcall callback (and (not answer) "0xseed") answer)))
                ((symbol-function 'lispm-remote--check-peers) #'ignore)
                ;; The conclusion comes from the failure alone.
                ((symbol-function 'url-retrieve)
                 (lambda (url &rest _) (ert-fail (format "LispM Remote asked %s" url)))))
        (lispm-remote--join)
        (should (= joins 1))
        (let ((home (lispm-remote-test--home-text)))
          (should (string-search "✖ Not on Rings: no seed runs this Rings version" home))
          (should (string-search "✖ seed.example runs another Rings version than this LispM Remote (0.36.0). Update LispM Remote, or join through a seed of Rings 0.36.0." home))
          (should (string-search "leaves seed.example alone for 30 minutes" home)))
        (should (eq (alist-get 'join (lispm-remote--progress '(:token "t"))) 'failed))
        ;; The checks every few seconds leave the seed alone.
        (dotimes (_ 5)
          (lispm-remote--join))
        (should (= joins 1))
        ;; Once the wait is over, LispM Remote tries once more.
        (setf (alist-get "https://seed.example" lispm-remote--mismatches nil nil #'equal)
              (list (- (float-time) lispm-remote-mismatch-seconds 1) nil))
        (lispm-remote--join)
        (should (= joins 2))
        (lispm-remote--join)
        (should (= joins 2))
        ;; A seed that lets it join is no longer named.
        (setf (alist-get "https://seed.example" lispm-remote--mismatches nil nil #'equal)
              (list (- (float-time) lispm-remote-mismatch-seconds 1) nil)
              answer nil)
        (lispm-remote--join)
        (should (= joins 3))
        (should-not lispm-remote--mismatches)
        (should-not (string-search "Rings version" (lispm-remote-test--home-text)))))))

(ert-deftest lispm-remote-test-another-seed-is-still-joined-through ()
  (lispm-remote-test--with-home
    (let ((lispm-remote-seeds '("https://seed.example" "https://other.example"))
          (joined nil))
      (cl-letf (((symbol-function 'lispm-remote-node-connect)
                 (lambda (_node seed callback)
                   (push seed joined)
                   (funcall callback nil (if (equal seed "https://seed.example")
                                             lispm-remote-test--mismatch
                                           "Joining took too long")))))
        (lispm-remote--join)
        (lispm-remote--join)
        (should (equal joined '("https://other.example" "https://other.example"
                                "https://seed.example")))
        (let ((home (lispm-remote-test--home-text)))
          (should (string-search "Joining Rings through other.example" home))
          (should (string-search "✖ seed.example runs another Rings version than this LispM Remote (0.36.0)." home)))
        (should (eq (alist-get 'join (lispm-remote--progress '(:token "t"))) 'active))))))

(ert-deftest lispm-remote-test-a-seed-conclusion-names-this-rings ()
  (let ((lispm-remote--rings "0.36.0")
        (lispm-remote--mismatches nil))
    (should (equal (lispm-remote--seed-conclusion "https://node.rings.rs/")
                   "node.rings.rs runs another Rings version than this LispM Remote (0.36.0). Update LispM Remote, or join through a seed of Rings 0.36.0."))
    ;; When the failure said which side is older.
    (setq lispm-remote--mismatches `(("https://node.rings.rs" ,(float-time) ours)))
    (should (equal (lispm-remote--seed-conclusion "https://node.rings.rs")
                   "node.rings.rs runs a newer Rings than this LispM Remote (0.36.0). Update LispM Remote."))
    (setq lispm-remote--mismatches `(("https://node.rings.rs" ,(float-time) seed)))
    (should (equal (lispm-remote--seed-conclusion "https://node.rings.rs")
                   "node.rings.rs runs an older Rings than this LispM Remote (0.36.0). Join through another seed.")))
  ;; Before the node said which Rings it runs.
  (let ((lispm-remote--rings nil)
        (lispm-remote--mismatches nil))
    (should (equal (lispm-remote--seed-conclusion "https://node.rings.rs")
                   "node.rings.rs runs another Rings version than this LispM Remote. Update LispM Remote, or join through another seed."))))

(ert-deftest lispm-remote-test-a-refusal-says-which-rings-is-older ()
  ;; A seed with a wire version refusing this node, from before wire
  ;; versioning: this node's Rings is older.
  (should (eq (lispm-remote--rings-older lispm-remote-test--refused) 'ours))
  ;; Seeds and nodes of two wire versions, either refusing.
  (let ((seed "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Incompatible wire version: ours is %d, the peer's is %d..")
        (node "Internal rpc services error: Server error: Incompatible wire version: ours is %d, the peer's is %d.."))
    (should (eq (lispm-remote--rings-older (format seed 2 1)) 'ours))
    (should (eq (lispm-remote--rings-older (format seed 1 2)) 'seed))
    (should (eq (lispm-remote--rings-older (format node 2 1)) 'seed))
    (should (eq (lispm-remote--rings-older (format node 1 2)) 'ours))
    (should-not (lispm-remote--rings-older (format node 2 2))))
  ;; This node refusing a seed from before wire versioning.
  (should (eq (lispm-remote--rings-older "Internal rpc services error: Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning..")
              'seed))
  ;; A codec error says nothing of which is older.
  (should-not (lispm-remote--rings-older lispm-remote-test--mismatch))
  (should-not (lispm-remote--rings-older nil)))

(ert-deftest lispm-remote-test-a-seed-with-a-wire-version-is-named-newer ()
  "A seed whose handshake carries a wire version refuses this node by
name: one failed join says that this LispM Remote is the older, and the
seed is left alone."
  (lispm-remote-test--with-home
    (let ((lispm-remote-seeds '("https://node.rings.rs"))
          (joins 0))
      (cl-letf (((symbol-function 'lispm-remote-node-connect)
                 (lambda (_node _seed callback)
                   (cl-incf joins)
                   (funcall callback nil lispm-remote-test--refused))))
        (lispm-remote--join)
        (lispm-remote--join)
        (should (= joins 1))
        (let ((home (lispm-remote-test--home-text)))
          (should (string-search "✖ node.rings.rs runs a newer Rings than this LispM Remote (0.36.0). Update LispM Remote." home))
          (should (string-search "leaves node.rings.rs alone for 30 minutes" home)))
        (should (eq (alist-get 'join (lispm-remote--progress '(:token "t"))) 'failed))))))

(ert-deftest lispm-remote-test-a-device-of-another-version-is-named ()
  (lispm-remote-test--with-home
    (lispm-remote--save-devices
     (list (list :peer lispm-remote-test--phone-did :key "00" :name "Test Phone"
                 :paired-at 0)))
    (lispm-remote--apply `((other-version ,lispm-remote-test--phone-did 2 1)
                           (other-version "0x3333333333333333333333333333333333333333" 1 2)))
    (let ((home (lispm-remote-test--home-text)))
      (should (string-search "✖ Test Phone has a newer LispM Remote page (protocol 2; this Emacs speaks 1). Update LispM Remote." home))
      (should (string-search "✖ A device has an older LispM Remote page (protocol 1; this Emacs speaks 2). Reload the page on it." home)))
    ;; A device heard again in the same version is named once.
    (lispm-remote--apply `((other-version ,lispm-remote-test--phone-did 2 1)))
    (should (= (length lispm-remote--peer-versions) 2))))

(ert-deftest lispm-remote-test-the-machines-name-the-other-version ()
  "LispM answers a page of a later version in its own, and says which the
page speaks; a page that hears LispM in another version stops there."
  (skip-unless (lispm-remote-node-test--module-p))
  (let* ((lispm-did "0x1111111111111111111111111111111111111111")
         (lispm (lispm-remote-core-responder (lispm-remote-core-generate-static) lispm-did nil))
         (phone (lispm-remote-core-initiator (lispm-remote-core-generate-static)
                                             lispm-remote-test--phone-did nil))
         (token (cadr (assq 'show-offer (lispm-remote-core-responder-offer lispm 120))))
         (init (nth 2 (assq 'transmit (lispm-remote-core-initiator-pair phone token "Phone")))))
    ;; The pairing request, as a page of the next version sends it.
    (aset init 0 2)
    (let* ((effects (lispm-remote-core-responder-receive lispm lispm-remote-test--phone-did init))
           (answer (nth 2 (assq 'transmit effects))))
      (should (equal (assq 'other-version effects)
                     `(other-version ,lispm-remote-test--phone-did 2 1)))
      (should (= (aref answer 0) 1))
      ;; The answer, as LispM of the next version sends it.
      (aset answer 0 2)
      (should (equal (lispm-remote-core-initiator-receive phone lispm-did answer)
                     '((failed other-version 2 1))))
      ;; The page sends its request no more.
      (should-not (lispm-remote-core-initiator-tick phone)))))

(ert-deftest lispm-remote-test-the-node-says-which-rings-it-runs ()
  (skip-unless (lispm-remote-node-test--module-p))
  (let ((lispm-remote-directory (make-temp-file "lispm-remote-test" t))
        (lispm-remote-seeds nil)
        (lispm-remote--rings nil)
        (built (with-temp-buffer
                 (insert-file-contents (expand-file-name "Cargo.toml" lispm-remote-module-root))
                 (re-search-forward "^rings-node = .*tag = \"v\\([^\"]+\\)\"")
                 (match-string 1))))
    (unwind-protect
        (progn
          (lispm-remote-mode 1)
          (should (equal (lispm-remote-node-test--wait (lambda () lispm-remote--rings)
                                                       "the node's Rings version")
                         built)))
      (lispm-remote-mode -1)
      (delete-directory lispm-remote-directory t))))

;;; lispm-remote-test.el ends here
