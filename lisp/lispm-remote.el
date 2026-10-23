;;; lispm-remote.el --- Reach a running LispM from your phone -*- lexical-binding: t; -*-

;; Copyright (C) 2026  Ryan.K

;; Author: Ryan.K
;; Version: 0.1.0
;; Package-Requires: ((emacs "30.1"))
;; Keywords: comm
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

;; LispM Remote keeps this Emacs reachable from your phone while it runs.
;; `lispm-remote-mode' starts a Rings Network node, joins the network
;; through `lispm-remote-seeds' and rejoins it whenever it loses every
;; peer.  Paired devices then reach LispM by its DID, from anywhere.
;;
;; To pair, run `lispm-remote-pair': it shows a QR code.  Open
;; lispm.ai/remote on the phone and scan it.  Both screens then show the
;; same fingerprint; press `a' to approve the phone, `r' to refuse it.  The
;; code works once, for `lispm-remote-offer-seconds'.  `lispm-remote-devices'
;; lists the paired devices; `d' there forgets one.
;;
;; Once a device opens a session, LispM sends it its state, and sends it
;; again whenever it changes.  The state is what `lispm-remote-state-function'
;; returns; by default, Emacs's version and LispM's agents: each one's name,
;; role, project, state and status, and under "approvals" the calls it waits
;; on, each with its "call" id, its "tool" and a short "summary".  Device and
;; LispM talk in JSON objects, sealed end to end:
;;
;;   LispM -> device   {"type": "state", "state": STATE}
;;   device -> LispM   {"type": "state?"}            ask for the state again
;;
;; A device controls LispM's agents with three commands:
;;
;;   {"type": "say", "agent": NAME, "text": TEXT}    give the agent the input TEXT
;;   {"type": "decide", "agent": NAME, "call": ID,
;;    "decision": "allow" | "always" | "deny"}     answer a call it waits on
;;   {"type": "interrupt", "agent": NAME}           stop the agent's turn
;;
;; "allow" runs the call, "always" runs it and calls like it for the rest
;; of the Emacs session, "deny" refuses it (with an optional "reason"): the
;; 1, 2 and 3 of LispM's approval prompt.  LispM answers each command:
;;
;;   {"type": "ack", "command": TYPE, "id": ID}
;;   {"type": "error", "command": TYPE, "id": ID, "message": TEXT}
;;
;; The "id" is the command's own, if it has one; LispM answers a command
;; repeated with the same id again without doing it twice, so a device may
;; send a command until it hears back.  Without LispM, every command is
;; answered with an error.  Other messages from devices go to
;; `lispm-remote-message-functions'.
;;
;; The keys are kept in `lispm-remote-directory', readable only by you: the
;; Rings account, this Emacs's static key, and the paired devices.

;;; Code:

(require 'cl-lib)
(require 'seq)
(require 'subr-x)
(require 'tabulated-list)
(require 'lispm-remote-node)

(declare-function lispm-remote-core-generate-static "lispm-remote-core")
(declare-function lispm-remote-core-responder "lispm-remote-core")
(declare-function lispm-remote-core-responder-receive "lispm-remote-core")
(declare-function lispm-remote-core-responder-offer "lispm-remote-core")
(declare-function lispm-remote-core-responder-approve "lispm-remote-core")
(declare-function lispm-remote-core-responder-refuse "lispm-remote-core")
(declare-function lispm-remote-core-responder-withdraw "lispm-remote-core")
(declare-function lispm-remote-core-responder-send "lispm-remote-core")
(declare-function lispm-remote-core-responder-forget "lispm-remote-core")
(declare-function lispm-remote-core-responder-tick "lispm-remote-core")
(declare-function lispm-remote-core-qr "lispm-remote-core")
(declare-function lispm-agents "lispm-core")
(declare-function lispm-get-agent "lispm-core")
(declare-function lispm-agent-name "lispm-core")
(declare-function lispm-agent-role "lispm-core")
(declare-function lispm-agent-root "lispm-core")
(declare-function lispm-agent-state "lispm-core")
(declare-function lispm-agent-status "lispm-core")
(declare-function lispm-agent-pending "lispm-core")
(declare-function lispm-tell "lispm-runtime")
(declare-function lispm-waiting-calls "lispm-runtime")
(declare-function lispm-asked "lispm-runtime")
(declare-function lispm-approve-for "lispm-runtime")
(declare-function lispm-allow "lispm-runtime")
(declare-function lispm-deny-for "lispm-runtime")
(declare-function lispm-abort "lispm-runtime")

;;;; Options

(defcustom lispm-remote-seeds '("https://node.rings.rs")
  "The seed nodes this Emacs joins Rings Network through."
  :type '(repeat string)
  :group 'lispm-remote)

(defcustom lispm-remote-page "https://lispm.ai/remote/"
  "The page devices open to pair; the QR code points at it."
  :type 'string
  :group 'lispm-remote)

(defcustom lispm-remote-offer-seconds 120
  "How long a pairing code works."
  :type 'natnum
  :group 'lispm-remote)

(defcustom lispm-remote-check-seconds 10
  "How often LispM Remote checks the network, rejoining it when the node
has lost every peer."
  :type 'natnum
  :group 'lispm-remote)

(defcustom lispm-remote-state-function #'lispm-remote-default-state
  "Function of no arguments returning the state devices show.
The value must be JSON as `json-serialize' takes it: alists for objects,
vectors for arrays."
  :type 'function
  :group 'lispm-remote)

(defvar lispm-remote-message-functions nil
  "Abnormal hook run with PEER, CHANNEL and MESSAGE for each message from
a device that LispM Remote does not answer itself.  MESSAGE is the JSON
object, parsed into an alist.")

(defvar lispm-remote-payload-functions nil
  "Abnormal hook run with PEER, CHANNEL and PAYLOAD for each payload from
a device that is not a JSON message.  PAYLOAD is a unibyte string.  The
first function that returns non-nil takes it.

A payload is told apart by its first byte: `{' starts a JSON message,
1 a message of LispM's display protocol (lispm-remote-display.el), and
every other first byte is reserved.  Send payloads with
`lispm-remote-send'.")

(defvar lispm-remote-session-functions nil
  "Abnormal hook run with PEER, CHANNEL and `opened' or `closed' as a
device's session opens or closes.  A device that comes back closes its
old session and opens another.")

;;;; The mode's state

(defvar lispm-remote--node nil
  "The Rings node, while `lispm-remote-mode' is on.")

(defvar lispm-remote--machine nil
  "The pairing and session machine, while `lispm-remote-mode' is on.")

(defvar lispm-remote--timer nil
  "The timer checking the network.")

(defvar lispm-remote--state-timer nil
  "The timer that sends a changed state shortly after the change.")

(defvar lispm-remote--sessions nil
  "The open sessions: a list of (PEER CHANNEL . STATE-SENT).")

(defvar lispm-remote--answers nil
  "The answers to the latest commands that had an id: an alist of
\((PEER . ID) . ANSWER), newest first.")

(defconst lispm-remote--answers-kept 64
  "How many answers to commands LispM Remote keeps, for repeats.")

(defvar lispm-remote--offer nil
  "The offer on show: a plist of :token, :name and :fingerprint of the
device asking, :ended with how it ended, and :paired with whom.")

(defconst lispm-remote--buffer "*LispM Remote*"
  "The buffer pairing is shown in.")

(defconst lispm-remote--log-buffer " *lispm-remote log*"
  "The buffer LispM Remote notes what it discarded and what failed in.")

(defun lispm-remote--log (format-string &rest arguments)
  "Note FORMAT-STRING with ARGUMENTS in the log buffer."
  (with-current-buffer (get-buffer-create lispm-remote--log-buffer)
    (goto-char (point-max))
    (insert (format-time-string "%T ") (apply #'format format-string arguments) "\n")))

;;;; Files

(defun lispm-remote--file (name)
  "Return the file NAME in `lispm-remote-directory'."
  (expand-file-name name lispm-remote-directory))

(defun lispm-remote--write-private (name text)
  "Write TEXT to the file NAME in `lispm-remote-directory', readable only
by you."
  (with-file-modes #o700
    (make-directory lispm-remote-directory t))
  (with-file-modes #o600
    (with-temp-file (lispm-remote--file name)
      (insert text))))

(defun lispm-remote--read (name)
  "Return the contents of the file NAME in `lispm-remote-directory', or
nil when there is none."
  (let ((file (lispm-remote--file name)))
    (when (file-readable-p file)
      (with-temp-buffer
        (insert-file-contents file)
        (buffer-string)))))

(defun lispm-remote--static ()
  "Return this Emacs's static secret, making and keeping one the first time."
  (or (when-let* ((text (lispm-remote--read "static")))
        (string-trim text))
      (let ((secret (lispm-remote-core-generate-static)))
        (lispm-remote--write-private "static" (concat secret "\n"))
        secret)))

(defun lispm-remote--devices ()
  "Return the paired devices: plists of :peer, :key, :name and :paired-at."
  (when-let* ((text (lispm-remote--read "devices.eld")))
    (car (read-from-string text))))

(defun lispm-remote--save-devices (devices)
  "Keep DEVICES as the paired devices."
  (lispm-remote--write-private
   "devices.eld" (concat ";; The devices paired with LispM Remote.\n"
                         (prin1-to-string devices) "\n")))

(defun lispm-remote--device-vector (device)
  "Return DEVICE as the machine takes it: [PEER KEY NAME PAIRED-AT]."
  (vector (plist-get device :peer) (plist-get device :key)
          (plist-get device :name) (plist-get device :paired-at)))

;;;; Starting and stopping

;;;###autoload
(define-minor-mode lispm-remote-mode
  "Keep LispM reachable from paired devices while Emacs runs."
  :global t
  :group 'lispm-remote
  (if lispm-remote-mode
      (condition-case error
          (lispm-remote--start)
        (error (setq lispm-remote-mode nil)
               (lispm-remote--stop)
               (signal (car error) (cdr error))))
    (lispm-remote--stop)))

(defun lispm-remote--start ()
  "Start the node and the machine, and join the network."
  (setq lispm-remote--node (lispm-remote-node-start nil #'lispm-remote--frame))
  (setq lispm-remote--machine
        (lispm-remote-core-responder
         (lispm-remote--static) (lispm-remote-node-did lispm-remote--node)
         (mapcar #'lispm-remote--device-vector (lispm-remote--devices))))
  (setq lispm-remote--timer (run-at-time 0 lispm-remote-check-seconds #'lispm-remote--check))
  (add-hook 'lispm-state-change-functions #'lispm-remote-state-changed))

(defun lispm-remote--stop ()
  "Stop the node and forget the sessions."
  (remove-hook 'lispm-state-change-functions #'lispm-remote-state-changed)
  (dolist (timer (list lispm-remote--timer lispm-remote--state-timer))
    (when timer (cancel-timer timer)))
  (when lispm-remote--node
    (lispm-remote-node-stop lispm-remote--node))
  (setq lispm-remote--node nil lispm-remote--machine nil lispm-remote--timer nil
        lispm-remote--state-timer nil lispm-remote--sessions nil lispm-remote--offer nil
        lispm-remote--answers nil))

(defun lispm-remote--check ()
  "Let an expired offer end, send a changed state, and rejoin the network
when the node has lost every peer."
  (when lispm-remote--machine
    (lispm-remote--apply (lispm-remote-core-responder-tick lispm-remote--machine))
    (lispm-remote--send-state)
    (lispm-remote-node-request
     lispm-remote--node "listPeers" nil
     (lambda (result error)
       (unless (or error
                   (seq-some (lambda (peer) (equal (alist-get 'state peer) "Connected"))
                             (alist-get 'peers result)))
         (lispm-remote--join))))))

(defun lispm-remote--join ()
  "Join the network through `lispm-remote-seeds'."
  (dolist (seed lispm-remote-seeds)
    (lispm-remote-node-connect
     lispm-remote--node seed
     (lambda (_did error)
       (when error (lispm-remote--log "joining through %s failed: %s" seed error))))))

;;;; Frames and effects

(defun lispm-remote--frame (from frame)
  "Feed the machine FRAME, which the peer FROM sent."
  (when lispm-remote--machine
    (lispm-remote--apply (lispm-remote-core-responder-receive lispm-remote--machine from frame))))

(defun lispm-remote--apply (effects)
  "Carry out EFFECTS, which the machine returned."
  (dolist (effect effects)
    (pcase effect
      (`(transmit ,to ,frame)
       (lispm-remote-node-send lispm-remote--node to frame
                               (lambda (_sent error)
                                 (when error (lispm-remote--log "sending to %s failed: %s" to error)))))
      (`(show-offer ,token)
       (setq lispm-remote--offer (list :token token))
       (lispm-remote--render))
      (`(ask-owner ,name ,fingerprint)
       (setq lispm-remote--offer (plist-put (plist-put lispm-remote--offer :name name)
                                            :fingerprint fingerprint))
       (lispm-remote--render)
       (message "LispM Remote: %s asks to pair, showing %s" name fingerprint))
      (`(offer-ended ,reason)
       (unless (eq reason 'used)
         (setq lispm-remote--offer (plist-put lispm-remote--offer :ended reason)))
       (lispm-remote--render))
      (`(paired ,peer ,key ,name ,paired-at)
       (lispm-remote--save-devices
        (cons (list :peer peer :key key :name name :paired-at paired-at)
              (seq-remove (lambda (device) (equal (plist-get device :peer) peer))
                          (lispm-remote--devices))))
       (setq lispm-remote--offer (plist-put lispm-remote--offer :paired name))
       (lispm-remote--render)
       (message "LispM Remote: paired with %s" name))
      (`(forgotten ,peer)
       (lispm-remote--save-devices
        (seq-remove (lambda (device) (equal (plist-get device :peer) peer))
                    (lispm-remote--devices))))
      (`(opened ,peer ,channel)
       (push (cons peer (cons channel nil)) lispm-remote--sessions)
       (lispm-remote--send-state peer channel)
       (run-hook-with-args 'lispm-remote-session-functions peer channel 'opened))
      (`(delivered ,peer ,channel ,payload)
       (lispm-remote--receive peer channel payload))
      (`(closed ,peer ,channel ,_reason)
       (setq lispm-remote--sessions
             (seq-remove (lambda (session) (lispm-remote--session-p session peer channel))
                         lispm-remote--sessions))
       (run-hook-with-args 'lispm-remote-session-functions peer channel 'closed))
      (`(discarded ,from ,why)
       (lispm-remote--log "discarded a frame from %s: %s" from why)))))

(defun lispm-remote--session-p (session peer channel)
  "Return non-nil when SESSION is the channel CHANNEL of PEER."
  (and (equal (car session) peer) (equal (cadr session) channel)))

;;;; State

(defun lispm-remote-default-state ()
  "Return Emacs's version and LispM's agents, with what each is doing."
  `((emacs . ,emacs-version)
    (agents . ,(vconcat (and (fboundp 'lispm-agents)
                             (mapcar #'lispm-remote--agent (lispm-agents)))))))

(defun lispm-remote--agent (agent)
  "Return LispM's AGENT as JSON."
  `((name . ,(format "%s" (lispm-agent-name agent)))
    (role . ,(format "%s" (lispm-agent-role agent)))
    (project . ,(abbreviate-file-name (or (lispm-agent-root agent) "")))
    (state . ,(format "%s" (lispm-agent-state agent)))
    (status . ,(or (lispm-agent-status agent) ""))
    (approvals . ,(lispm-remote--approvals agent))))

(defconst lispm-remote--summary-length 300
  "The most characters of a call's summary.")

(defun lispm-remote--summary (input)
  "Return a short text saying what a call with INPUT, an alist, does."
  (let* ((input (and (consp input) input))
         (text (or (seq-some (lambda (key)
                               (let ((value (alist-get key input)))
                                 (and (stringp value) (not (string-blank-p value)) value)))
                             '(command code path buffer name url query))
                   (and input
                        (condition-case nil
                            (json-serialize input)
                          (error (prin1-to-string input))))
                   "")))
    (truncate-string-to-width (string-trim text) lispm-remote--summary-length nil nil "…")))

(defun lispm-remote--approvals (agent)
  "Return the calls LispM's AGENT waits on approval of, as JSON."
  (vconcat
   (and (fboundp 'lispm-waiting-calls) (fboundp 'lispm-asked)
        (mapcar (lambda (id)
                  (let ((asked (lispm-asked (gethash id (lispm-agent-pending agent)))))
                    `((call . ,id)
                      (tool . ,(format "%s" (plist-get asked :name)))
                      (summary . ,(lispm-remote--summary (plist-get asked :input))))))
                (lispm-waiting-calls agent)))))

(defun lispm-remote--send-message (peer channel message)
  "Send MESSAGE, JSON as `json-serialize' takes it, on CHANNEL to PEER."
  (lispm-remote--apply
   (lispm-remote-core-responder-send lispm-remote--machine peer channel
                                     (json-serialize message))))

(defun lispm-remote--send-state (&optional peer channel)
  "Send the state to every session it changed for since last sent, and to
the session CHANNEL of PEER whether or not it changed."
  (when lispm-remote--sessions
    (let* ((state (funcall lispm-remote-state-function))
           (json (json-serialize state)))
      (dolist (session lispm-remote--sessions)
        (when (or (lispm-remote--session-p session peer channel)
                  (not (equal (cddr session) json)))
          (setcdr (cdr session) json)
          (condition-case error
              (lispm-remote--send-message (car session) (cadr session)
                                          `((type . "state") (state . ,state)))
            (error (lispm-remote--log "sending the state failed: %s"
                                      (error-message-string error)))))))))

(defun lispm-remote-state-changed (&rest _)
  "Send the state to the devices shortly, once changes settle."
  (when (and lispm-remote--sessions (not lispm-remote--state-timer))
    (setq lispm-remote--state-timer
          (run-at-time 0.3 nil (lambda ()
                                 (setq lispm-remote--state-timer nil)
                                 (when lispm-remote--machine
                                   (lispm-remote--send-state)))))))

(defun lispm-remote-send (peer channel payload)
  "Send PAYLOAD, a unibyte string, on the session CHANNEL of PEER.
A payload must not start with `{' unless it is a JSON message; see
`lispm-remote-payload-functions'."
  (unless lispm-remote--machine
    (error "LispM Remote is off"))
  (lispm-remote--apply
   (lispm-remote-core-responder-send lispm-remote--machine peer channel payload)))

(defun lispm-remote--receive (peer channel payload)
  "Act on PAYLOAD, which PEER sent on CHANNEL."
  (if (and (> (length payload) 0) (/= (aref payload 0) ?{))
      (unless (run-hook-with-args-until-success 'lispm-remote-payload-functions
                                                peer channel payload)
        (lispm-remote--log "nobody takes a payload from %s starting with %d" peer (aref payload 0)))
    (lispm-remote--receive-json peer channel payload)))

(defun lispm-remote--receive-json (peer channel payload)
  "Act on PAYLOAD, a JSON message PEER sent on CHANNEL."
  (let ((message (condition-case nil
                     (json-parse-string (decode-coding-string payload 'utf-8)
                                        :object-type 'alist :array-type 'list
                                        :null-object nil :false-object nil)
                   (error nil))))
    (pcase (alist-get 'type message)
      ("state?" (lispm-remote--send-state peer channel))
      ((and type (or "say" "decide" "interrupt"))
       (lispm-remote--send-message peer channel (lispm-remote--answer peer type message)))
      ('nil (lispm-remote--log "a message from %s is not a JSON object" peer))
      (_ (run-hook-with-args 'lispm-remote-message-functions peer channel message)))))

;;;; Commands

(defun lispm-remote--answer (peer type message)
  "Return the answer to the command MESSAGE of TYPE from PEER, doing it
unless it repeats one already done."
  (let* ((id (alist-get 'id message))
         (key (and id (cons peer id))))
    (or (and key (cdr (assoc key lispm-remote--answers)))
        (let ((answer (lispm-remote--do type message)))
          (when key
            (setq lispm-remote--answers
                  (seq-take (cons (cons key answer) lispm-remote--answers)
                            lispm-remote--answers-kept)))
          answer))))

(defun lispm-remote--do (type message)
  "Do the command MESSAGE of TYPE, and return its answer, as JSON."
  (let ((id (and (alist-get 'id message) `((id . ,(alist-get 'id message))))))
    (condition-case error
        (progn
          (pcase type
            ("say" (lispm-remote-say (alist-get 'agent message) (alist-get 'text message)))
            ("decide" (lispm-remote-decide (alist-get 'agent message) (alist-get 'call message)
                                           (alist-get 'decision message)
                                           (alist-get 'reason message)))
            ("interrupt" (lispm-remote-interrupt (alist-get 'agent message))))
          `((type . "ack") (command . ,type) ,@id))
      (error
       `((type . "error") (command . ,type) ,@id
         (message . ,(error-message-string error)))))))

(defun lispm-remote--lispm-agent (name)
  "Return LispM's agent called NAME, a string, or signal a `user-error'."
  (unless (fboundp 'lispm-get-agent)
    (user-error "LispM is not running in this Emacs"))
  (unless (and (stringp name) (not (string-empty-p name)))
    (user-error "Which agent?"))
  (or (lispm-get-agent name t)
      (user-error "No agent is called %s" name)))

(defun lispm-remote-say (agent text)
  "Give LispM's AGENT, named by a string, the input TEXT, as a device does."
  (let ((agent (lispm-remote--lispm-agent agent)))
    (unless (and (stringp text) (not (string-blank-p text)))
      (user-error "There is nothing to say"))
    (lispm-tell agent text)))

(defun lispm-remote-decide (agent call decision &optional reason)
  "Answer the CALL that LispM's AGENT, named by a string, waits on.
DECISION is \"allow\" to run it, \"always\" to run it and calls like it
for the rest of the Emacs session, or \"deny\" to refuse it, for REASON
if given."
  (let ((agent (lispm-remote--lispm-agent agent)))
    (unless (and (stringp call) (member call (lispm-waiting-calls agent)))
      (user-error "%s waits on no call %s" (lispm-agent-name agent) call))
    (pcase decision
      ("allow" (lispm-approve-for agent call))
      ("always" (lispm-allow agent call))
      ("deny" (lispm-deny-for agent (and (stringp reason) reason) call))
      (_ (user-error "A decision is allow, always or deny, not %s" decision)))))

(defun lispm-remote-interrupt (agent)
  "Stop the turn of LispM's AGENT, named by a string."
  (lispm-abort (lispm-remote--lispm-agent agent)))

;;;; Pairing

(defun lispm-remote--require-mode ()
  "Turn on `lispm-remote-mode' unless it is on."
  (unless lispm-remote-mode
    (lispm-remote-mode 1)))

;;;###autoload
(defun lispm-remote-pair ()
  "Show a QR code a phone pairs with LispM by."
  (interactive)
  (lispm-remote--require-mode)
  (lispm-remote--apply (lispm-remote-core-responder-offer lispm-remote--machine
                                                          lispm-remote-offer-seconds))
  (pop-to-buffer (lispm-remote--render)))

(defun lispm-remote-approve (&optional fingerprint)
  "Pair the device asking, whose fingerprint the owner compared.
FINGERPRINT, when given, must be the one the device shows."
  (interactive)
  (let ((asked (plist-get lispm-remote--offer :fingerprint)))
    (unless asked
      (user-error "No device asks to pair"))
    (lispm-remote--apply
     (lispm-remote-core-responder-approve lispm-remote--machine (or fingerprint asked)))))

(defun lispm-remote-refuse ()
  "Turn the device asking to pair away."
  (interactive)
  (unless (plist-get lispm-remote--offer :fingerprint)
    (user-error "No device asks to pair"))
  (lispm-remote--apply (lispm-remote-core-responder-refuse lispm-remote--machine)))

(defun lispm-remote-withdraw ()
  "Stop the pairing code working, and close its window."
  (interactive)
  (when (and lispm-remote--machine lispm-remote--offer
             (not (plist-get lispm-remote--offer :ended))
             (not (plist-get lispm-remote--offer :paired)))
    (lispm-remote--apply (lispm-remote-core-responder-withdraw lispm-remote--machine)))
  (quit-window))

(defvar-keymap lispm-remote-pair-mode-map
  :doc "Keys of the pairing buffer."
  "a" #'lispm-remote-approve
  "r" #'lispm-remote-refuse
  "g" #'lispm-remote-pair
  "q" #'lispm-remote-withdraw)

(define-derived-mode lispm-remote-pair-mode special-mode "LispM Remote"
  "Mode of the buffer a phone pairs with LispM in."
  (setq-local cursor-type nil))

(defun lispm-remote--qr-rows ()
  "Return the rows of the offer's QR code, strings of 1 and 0, with a
quiet zone of four modules."
  (let* ((rows (lispm-remote-core-qr (concat lispm-remote-page "#"
                                             (plist-get lispm-remote--offer :token))))
         (blank (make-string (+ 8 (length (car rows))) ?0))
         (quiet (make-list 4 blank)))
    (append quiet (mapcar (lambda (row) (concat "0000" row "0000")) rows) quiet)))

(defun lispm-remote--qr-svg (rows size)
  "Return ROWS as an SVG image SIZE pixels a side."
  (let ((modules (length rows)))
    (with-temp-buffer
      (insert (format "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" viewBox=\"0 0 %d %d\" shape-rendering=\"crispEdges\"><rect width=\"%d\" height=\"%d\" fill=\"#fff\"/><path fill=\"#000\" d=\""
                      size size modules modules modules modules))
      (cl-loop for row in rows for y from 0
               do (cl-loop for module across row for x from 0
                           when (eq module ?1)
                           do (insert (format "M%d %dh1v1h-1z" x y))))
      (insert "\"/></svg>")
      (buffer-string))))

(defun lispm-remote--qr-text (rows)
  "Return ROWS as text, two rows a line, in half blocks, dark on light."
  (mapconcat
   (lambda (pair)
     (propertize
      (apply #'string
             (cl-loop for top across (car pair) for x from 0
                      collect (pcase (cons (eq top ?1)
                                           (and (cadr pair) (eq (aref (cadr pair) x) ?1)))
                                ('(t . t) ?█) ('(t . nil) ?▀) ('(nil . t) ?▄) (_ ?\s))))
      'face '(:foreground "black" :background "white")))
   (seq-partition rows 2) "\n"))

(defun lispm-remote-write-qr (file &optional scale)
  "Write the offer's QR code to FILE as a PBM image, SCALE pixels a
module (8 by default), for programs that read images."
  (let* ((scale (or scale 8))
         (rows (lispm-remote--qr-rows))
         (side (* scale (length rows))))
    (with-temp-file file
      (insert (format "P1\n%d %d\n" side side))
      (dolist (row rows)
        (let ((line (mapconcat (lambda (module) (make-string scale (if (eq module ?1) ?1 ?0)))
                               row "")))
          (dotimes (_ scale)
            (insert line "\n")))))))

(defun lispm-remote--render ()
  "Show the state of pairing in its buffer, and return the buffer."
  (let ((buffer (get-buffer-create lispm-remote--buffer))
        (offer lispm-remote--offer))
    (with-current-buffer buffer
      (unless (derived-mode-p 'lispm-remote-pair-mode)
        (lispm-remote-pair-mode))
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert (propertize "Pair a phone with LispM" 'face 'bold) "\n\n")
        (cond
         ((plist-get offer :paired)
          (insert (format "Paired with %s.\n\nIt can reach LispM whenever Emacs runs.\n"
                          (plist-get offer :paired))))
         ((plist-get offer :ended)
          (insert (pcase (plist-get offer :ended)
                    ('expired "The code expired.")
                    ('refused "You refused the device.")
                    ('contested "Two devices used the code, so it stopped working: someone else may have seen it.")
                    ('withdrawn "You withdrew the code.")
                    ('replaced "A newer code replaced this one.")
                    (_ "The code stopped working."))
                  "\n\nPress g for a new code.\n"))
         ((plist-get offer :fingerprint)
          (insert (format "%s asks to pair.\n\nIts screen shows  %s\n\n"
                          (plist-get offer :name)
                          (propertize (plist-get offer :fingerprint) 'face 'bold))
                  "If your phone shows the same, press a to approve it, r to refuse it.\n"))
         ((plist-get offer :token)
          (let ((rows (lispm-remote--qr-rows)))
            (if (and (display-graphic-p) (image-type-available-p 'svg))
                (insert-image (create-image (lispm-remote--qr-svg rows 360) 'svg t))
              (insert (lispm-remote--qr-text rows)))
            (insert (format "\n\nOn your phone, open %s and scan this code.\nIt works once, for %d minutes.\n"
                            (string-remove-suffix "/" (string-remove-prefix "https://" lispm-remote-page))
                            (max 1 (/ lispm-remote-offer-seconds 60))))))
         (t (insert "No code yet. Press g for one.\n")))
        (goto-char (point-min))))
    buffer))

;;;; Devices

(defun lispm-remote-forget (peer)
  "Forget the device PEER and close its sessions."
  (lispm-remote--require-mode)
  (lispm-remote--apply (lispm-remote-core-responder-forget lispm-remote--machine peer)))

(defvar-keymap lispm-remote-devices-mode-map
  :doc "Keys of the device list."
  "d" #'lispm-remote-devices-forget)

(define-derived-mode lispm-remote-devices-mode tabulated-list-mode "LispM Devices"
  "Mode of the list of devices paired with LispM."
  (setq tabulated-list-format [("Device" 24 t) ("Paired" 18 t) ("DID" 0 nil)])
  (setq tabulated-list-sort-key '("Paired" . t))
  (add-hook 'tabulated-list-revert-hook #'lispm-remote--devices-refresh nil t)
  (tabulated-list-init-header))

(defun lispm-remote--devices-refresh ()
  "Fill the device list."
  (setq tabulated-list-entries
        (mapcar (lambda (device)
                  (list (plist-get device :peer)
                        (vector (plist-get device :name)
                                (format-time-string "%F %R" (/ (plist-get device :paired-at) 1000.0))
                                (plist-get device :peer))))
                (lispm-remote--devices))))

;;;###autoload
(defun lispm-remote-devices ()
  "List the devices paired with LispM."
  (interactive)
  (with-current-buffer (get-buffer-create "*LispM Devices*")
    (lispm-remote-devices-mode)
    (lispm-remote--devices-refresh)
    (tabulated-list-print)
    (pop-to-buffer (current-buffer))))

(defun lispm-remote-devices-forget ()
  "Forget the device at point, after asking."
  (interactive)
  (when-let* ((peer (tabulated-list-get-id))
              ((yes-or-no-p (format "Forget %s? " (aref (tabulated-list-get-entry) 0)))))
    (lispm-remote-forget peer)
    (revert-buffer)))

(provide 'lispm-remote)
;;; lispm-remote.el ends here
