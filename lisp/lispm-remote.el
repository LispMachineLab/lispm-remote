;;; lispm-remote.el --- Reach a running LispM from your phone or another computer -*- lexical-binding: t; -*-

;; Copyright (C) 2026  Ryan.K

;; Author: Ryan.K
;; Version: 0.2.0
;; Package-Requires: ((emacs "30.1") (lispm-prebuilt "0.1.0"))
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

;; LispM Remote keeps this Emacs reachable from your phone or another
;; computer while it runs.
;; `lispm-remote-mode' starts a Rings Network node, joins the network
;; through `lispm-remote-seeds' and rejoins it whenever it loses every
;; peer.  Paired devices then reach LispM by its DID, from anywhere.
;;
;; When the other end runs another version, LispM Remote says so at once,
;; in its window (`M-x lispm-remote') and in the echo area, and does not
;; keep trying: a seed of another Rings version is left alone for
;; `lispm-remote-mismatch-seconds', and a device of another version of
;; LispM Remote's protocol is told which version this one speaks.
;;
;; To pair, run `lispm-remote-pair': it shows a QR code.  Open
;; lispm.ai/remote on the device and scan it.  Both screens then show the
;; same fingerprint; press `a' to approve the device, `r' to refuse it.  The
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
;; answered with an error.
;;
;; A device follows one agent's conversation at a time, as a chat:
;;
;;   device -> LispM   {"type": "watch", "agent": NAME}   follow NAME's conversation
;;   device -> LispM   {"type": "watch"}                  follow none
;;   LispM -> device   {"type": "chat", "agent": NAME, "items": [ITEM...], "more": N}
;;
;; LispM answers "watch" as a command, sends the conversation at once, and
;; sends it again whenever it changes, the response streaming in included.
;; It sends the latest items that fit one message, oldest first; N earlier
;; ones stay in Emacs.  An item is an object with its "kind", as LispM's
;; chat has it: "user" (with "from" when an agent or LispM wrote it),
;; "text", "thought", "tool" (with "name", "state", a "summary" of its input
;; and its "result"), "note", "event" or "summary".  "watch" changes
;; nothing but what is sent, so a device sends it until a chat comes.
;; Other messages from devices go to `lispm-remote-message-functions'.
;;
;; The keys are kept in `lispm-remote-directory', readable only by you: the
;; Rings account, this Emacs's static key, and the paired devices.

;;; Code:

(require 'cl-lib)
(require 'seq)
(require 'subr-x)
(require 'svg)
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
(declare-function lispm-agent-buffer "lispm-core")
(declare-function lispm-chat-items "lispm-chat")

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

(defcustom lispm-remote-mismatch-seconds 1800
  "How long LispM Remote leaves a seed alone that runs another Rings
version, before it tries to join through it again.  Turning
`lispm-remote-mode' off and on tries at once, as does a seed newly in
`lispm-remote-seeds'."
  :type 'natnum
  :group 'lispm-remote)

(defcustom lispm-remote-chat-items 40
  "The most items of an agent's conversation a device is sent at once."
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

(cl-defstruct (lispm-remote--session (:constructor lispm-remote--session-create)
                                     (:predicate nil)
                                     (:copier nil))
  "An open session with a device: its PEER and CHANNEL, the STATE last sent
to it, the agent whose conversation it WATCHes, and the CHAT last sent."
  peer channel state watch chat)

(defvar lispm-remote--sessions nil
  "The open sessions, newest first.")

(defvar lispm-remote--answers nil
  "The answers to the latest commands that had an id: an alist of
\((PEER . ID) . ANSWER), newest first.")

(defconst lispm-remote--answers-kept 64
  "How many answers to commands LispM Remote keeps, for repeats.")

(defvar lispm-remote--offer nil
  "The offer on show: a plist of :token, :shown and :expires (times),
:name and :fingerprint of the device asking, :ended with how it ended,
:paired and :paired-peer with whom, :opened once that device opened a
session, and the drawing of the code, :rows and :image.")

(defvar lispm-remote--peers nil
  "How many peers the node was connected to when last checked, or nil
before the first check.")

(defvar lispm-remote--rings nil
  "The Rings version this Emacs's node runs, as \"0.36.0\", once known.")

(defvar lispm-remote--mismatches nil
  "The seeds that run another Rings version: an alist of (SEED . TIME),
TIME when a join through SEED last failed so.")

(defvar lispm-remote--peer-versions nil
  "The devices heard speaking another version of LispM Remote's protocol:
a list of (PEER VERSION OURS), newest first.")

(defconst lispm-remote--buffer "*LispM Remote*"
  "The buffer LispM Remote's window shows: its home, or pairing.")

(defvar lispm-remote--view 'home
  "What LispM Remote's window shows: `home' or `pair'.")

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

(defconst lispm-remote--change-hooks
  '(lispm-state-change-functions lispm-response-delta-functions lispm-turn-end-functions)
  "LispM's hooks that say what devices are sent may have changed.")

(defun lispm-remote--start ()
  "Start the node and the machine, and join the network."
  ;; A paired device opens LispM's windows over its session.
  (require 'lispm-remote-display nil t)
  (setq lispm-remote--node (lispm-remote-node-start nil #'lispm-remote--frame))
  (setq lispm-remote--machine
        (lispm-remote-core-responder
         (lispm-remote--static) (lispm-remote-node-did lispm-remote--node)
         (mapcar #'lispm-remote--device-vector (lispm-remote--devices))))
  ;; Which Rings it runs, to say so when a seed runs another.
  (lispm-remote-node-request
   lispm-remote--node "nodeInfo" nil
   (lambda (result _error)
     (setq lispm-remote--rings (lispm-remote--version-number (alist-get 'version result)))))
  (setq lispm-remote--timer (run-at-time 0 lispm-remote-check-seconds #'lispm-remote--check))
  (dolist (hook lispm-remote--change-hooks)
    (add-hook hook #'lispm-remote-state-changed)))

(defun lispm-remote--stop ()
  "Stop the node and forget the sessions."
  (dolist (hook lispm-remote--change-hooks)
    (remove-hook hook #'lispm-remote-state-changed))
  (dolist (timer (list lispm-remote--timer lispm-remote--state-timer))
    (when timer (cancel-timer timer)))
  (when lispm-remote--node
    (lispm-remote-node-stop lispm-remote--node))
  (setq lispm-remote--node nil lispm-remote--machine nil lispm-remote--timer nil
        lispm-remote--state-timer nil lispm-remote--sessions nil lispm-remote--offer nil
        lispm-remote--answers nil lispm-remote--peers nil lispm-remote--mismatches nil
        lispm-remote--peer-versions nil))

(defun lispm-remote--check ()
  "Let an expired offer end, send a changed state, and rejoin the network
when the node has lost every peer."
  (when lispm-remote--machine
    (lispm-remote--apply (lispm-remote-core-responder-tick lispm-remote--machine))
    (lispm-remote--send-state)
    (lispm-remote--send-chats)
    (lispm-remote--check-peers t)))

(defun lispm-remote--check-peers (&optional rejoin)
  "Count the node's peers, redrawing the pairing screen when the count
changed; with REJOIN, join the network again when there are none."
  (when lispm-remote--node
    (lispm-remote-node-request
     lispm-remote--node "listPeers" nil
     (lambda (result error)
       (unless error
         (let ((count (seq-count (lambda (peer) (equal (alist-get 'state peer) "Connected"))
                                 (alist-get 'peers result))))
           (unless (eql count lispm-remote--peers)
             (setq lispm-remote--peers count)
             (lispm-remote--refresh))
           (when (and rejoin (zerop count))
             (lispm-remote--join))))))))

(defun lispm-remote--join ()
  "Join the network through `lispm-remote-seeds', but through none that
ran another Rings version lately."
  (dolist (seed (seq-remove #'lispm-remote--mismatched-p lispm-remote-seeds))
    (lispm-remote-node-connect
     lispm-remote--node seed
     (lambda (_did error)
       (cond ((not error)
              (setq lispm-remote--mismatches
                    (assoc-delete-all seed lispm-remote--mismatches))
              (lispm-remote--check-peers))
             (t
              (lispm-remote--log "joining through %s failed: %s" seed error)
              (when (lispm-remote-rings-mismatch-p error)
                (lispm-remote--mismatch seed))))))))

;;;; Versions

;; Each end LispM Remote talks to may run another version than this one.
;;
;; - The seed.  Rings's handshake carries no version: a seed of another
;;   Rings version only fails to read this node's offer, or this node its
;;   answer, and says so in words `lispm-remote-rings-mismatch-p' knows.
;;   Joining again would fail the same way, and a seed refuses an address
;;   that keeps trying, so LispM Remote leaves it alone for
;;   `lispm-remote-mismatch-seconds' and says what it concluded.  Which
;;   side is older it cannot tell until Rings's handshake carries the
;;   version.
;; - A device.  Every frame starts with the version of LispM Remote's
;;   protocol it is in.  The machine answers a frame of another version in
;;   this one, so the device learns which this one speaks, and says which
;;   the device speaks: `(other-version PEER VERSION OURS)'.

(defun lispm-remote--version-number (text)
  "Return the version TEXT starts with, as \"0.36.0\", or nil.
Rings's node says \"0.36.0-99feaaf\" when built from a checkout, and its
releases are tagged \"v0.36.0\"."
  (and (stringp text)
       (string-match "\\`v?\\([0-9]+\\(?:\\.[0-9]+\\)*\\)" text)
       (match-string 1 text)))

(defun lispm-remote-rings-mismatch-p (error)
  "Return non-nil when ERROR, why joining through a seed failed, says
that the seed runs another Rings version.
A seed that cannot read this node's offer answers \"Answer offer info
failed: Codec deserialization error\"; this node fails to read a seed's
answer with \"Decode error\"."
  (and (stringp error)
       (string-match-p "Codec deserialization error\\|\\bDecode error\\b" error)))

(defun lispm-remote--mismatched-p (seed)
  "Return non-nil when SEED ran another Rings version less than
`lispm-remote-mismatch-seconds' ago."
  (when-let* ((at (alist-get seed lispm-remote--mismatches nil nil #'equal)))
    (< (- (float-time) at) lispm-remote-mismatch-seconds)))

(defun lispm-remote--mismatch (seed)
  "Note that SEED runs another Rings version, and tell the owner the
first time."
  (let ((again (assoc seed lispm-remote--mismatches)))
    (setf (alist-get seed lispm-remote--mismatches nil nil #'equal) (float-time))
    (unless again
      (message "LispM Remote: %s" (lispm-remote--seed-conclusion seed)))
    (lispm-remote--refresh)))

(defun lispm-remote--host (seed)
  "Return SEED's host, as the owner knows it."
  (string-remove-suffix "/" (replace-regexp-in-string "\\`https?://" "" seed)))

(defun lispm-remote--seed-conclusion (seed)
  "Return what LispM Remote concludes from SEED running another Rings
version, and what to do.  Which side is older the seed does not say."
  (let ((host (lispm-remote--host seed))
        (ours lispm-remote--rings))
    (if ours
        (format "%s runs another Rings version than this LispM Remote (%s). Update LispM Remote, or join through a seed of Rings %s."
                host ours ours)
      (format "%s runs another Rings version than this LispM Remote. Update LispM Remote, or join through another seed."
              host))))

(defun lispm-remote--other-version (peer version ours)
  "Note that PEER speaks VERSION of the protocol, this end OURS, and tell
the owner the first time."
  (let ((again (equal (assoc peer lispm-remote--peer-versions) (list peer version ours))))
    (setq lispm-remote--peer-versions
          (cons (list peer version ours) (assoc-delete-all peer lispm-remote--peer-versions)))
    (lispm-remote--log "%s speaks protocol %d, this LispM Remote %d" peer version ours)
    (unless again
      (message "LispM Remote: %s" (lispm-remote--peer-conclusion peer version ours)))
    (lispm-remote--refresh)))

(defun lispm-remote--peer-conclusion (peer version ours)
  "Return what LispM Remote concludes from PEER speaking VERSION of the
protocol, this end OURS: which is older, and what to do."
  (let ((name (or (plist-get (seq-find (lambda (device) (equal (plist-get device :peer) peer))
                                       (lispm-remote--devices))
                             :name)
                  "A device")))
    (if (> version ours)
        (format "%s has a newer LispM Remote page (protocol %d; this Emacs speaks %d). Update LispM Remote."
                name version ours)
      (format "%s has an older LispM Remote page (protocol %d; this Emacs speaks %d). Reload the page on it."
              name version ours))))

(defun lispm-remote--stranded-p ()
  "Return non-nil when no seed is left to join through: every one ran
another Rings version lately."
  (and lispm-remote-seeds (seq-every-p #'lispm-remote--mismatched-p lispm-remote-seeds)))

(defun lispm-remote--insert-versions ()
  "Insert what LispM Remote concluded of the ends that run another
version, if any: seeds while it is off the network, and devices."
  (unless (and lispm-remote--peers (> lispm-remote--peers 0))
    (dolist (seed (seq-filter #'lispm-remote--mismatched-p lispm-remote-seeds))
      (insert (propertize (concat "✖ " (lispm-remote--seed-conclusion seed))
                          'face 'lispm-remote-failed)
              "\n"
              (propertize (format "  LispM Remote leaves %s alone for %d minutes; turn it off and on to try at once."
                                  (lispm-remote--host seed)
                                  (/ lispm-remote-mismatch-seconds 60))
                          'face 'lispm-remote-pending)
              "\n")))
  (pcase-dolist (`(,peer ,version ,ours) lispm-remote--peer-versions)
    (insert (propertize (concat "✖ " (lispm-remote--peer-conclusion peer version ours))
                        'face 'lispm-remote-failed)
            "\n")))

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
       (when-let* ((image (plist-get lispm-remote--offer :image)))
         (lispm-remote--flush image))
       (setq lispm-remote--offer
             (list :token token :shown (float-time)
                   :expires (+ (float-time) lispm-remote-offer-seconds)))
       (lispm-remote--render))
      (`(ask-owner ,name ,fingerprint)
       (setq lispm-remote--offer (plist-put (plist-put lispm-remote--offer :name name)
                                            :fingerprint fingerprint)
             lispm-remote--view 'pair)
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
       (setq lispm-remote--offer (plist-put (plist-put lispm-remote--offer :paired name)
                                            :paired-peer peer))
       (lispm-remote--render)
       (message "LispM Remote: paired with %s" name))
      (`(forgotten ,peer)
       (lispm-remote--save-devices
        (seq-remove (lambda (device) (equal (plist-get device :peer) peer))
                    (lispm-remote--devices)))
       (lispm-remote--refresh))
      (`(opened ,peer ,channel)
       ;; A session opened, so the device speaks this version now.
       (setq lispm-remote--peer-versions (assoc-delete-all peer lispm-remote--peer-versions))
       (push (lispm-remote--session-create :peer peer :channel channel) lispm-remote--sessions)
       (lispm-remote--send-state peer channel)
       (if (equal peer (plist-get lispm-remote--offer :paired-peer))
           (progn (setq lispm-remote--offer (plist-put lispm-remote--offer :opened t))
                  (lispm-remote--render))
         (lispm-remote--refresh))
       (run-hook-with-args 'lispm-remote-session-functions peer channel 'opened))
      (`(delivered ,peer ,channel ,payload)
       (lispm-remote--receive peer channel payload))
      (`(closed ,peer ,channel ,_reason)
       (setq lispm-remote--sessions
             (seq-remove (lambda (session) (lispm-remote--session-p session peer channel))
                         lispm-remote--sessions))
       (lispm-remote--refresh)
       (run-hook-with-args 'lispm-remote-session-functions peer channel 'closed))
      (`(discarded ,from ,why)
       (lispm-remote--log "discarded a frame from %s: %s" from why))
      (`(other-version ,from ,version ,ours)
       (lispm-remote--other-version from version ours)))))

(defun lispm-remote--session-p (session peer channel)
  "Return non-nil when SESSION is the channel CHANNEL of PEER."
  (and (equal (lispm-remote--session-peer session) peer)
       (equal (lispm-remote--session-channel session) channel)))

(defun lispm-remote--session (peer channel)
  "Return the session on the channel CHANNEL of PEER, if it is open."
  (seq-find (lambda (session) (lispm-remote--session-p session peer channel))
            lispm-remote--sessions))

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

(defun lispm-remote--send-json (peer channel json)
  "Send JSON, a serialized object, on CHANNEL to PEER."
  (lispm-remote--apply
   (lispm-remote-core-responder-send lispm-remote--machine peer channel json)))

(defun lispm-remote--send-message (peer channel message)
  "Send MESSAGE, JSON as `json-serialize' takes it, on CHANNEL to PEER."
  (lispm-remote--send-json peer channel (json-serialize message)))

(defun lispm-remote--send-changed (slot make &optional peer channel)
  "Send each session the message MAKE, a function, makes of it, unless that
is nil, when it differs from the one the session's SLOT says was sent
last; and to the session CHANNEL of PEER whether or not."
  (dolist (session lispm-remote--sessions)
    (when-let* ((message (funcall make session))
                (json (json-serialize message))
                ((or (lispm-remote--session-p session peer channel)
                     (not (equal (cl-struct-slot-value 'lispm-remote--session slot session)
                                 json)))))
      (setf (cl-struct-slot-value 'lispm-remote--session slot session) json)
      (condition-case error
          (lispm-remote--send-json (lispm-remote--session-peer session)
                                   (lispm-remote--session-channel session) json)
        (error (lispm-remote--log "sending the %s to %s failed: %s" slot
                                  (lispm-remote--session-peer session)
                                  (error-message-string error)))))))

(defun lispm-remote--send-state (&optional peer channel)
  "Send the state to every session it changed for since last sent, and to
the session CHANNEL of PEER whether or not it changed."
  (when lispm-remote--sessions
    (let ((message `((type . "state") (state . ,(funcall lispm-remote-state-function)))))
      (lispm-remote--send-changed 'state (lambda (_) message) peer channel))))

(defun lispm-remote--send-chats (&optional peer channel)
  "Send each session that watches an agent the agent's conversation, when
it changed since last sent; to the session CHANNEL of PEER whether or not."
  (let (chats)
    (lispm-remote--send-changed
     'chat
     (lambda (session)
       (when-let* ((agent (lispm-remote--session-watch session)))
         (with-memoization (alist-get agent chats nil nil #'equal)
           (lispm-remote--chat agent))))
     peer channel)))

(defun lispm-remote-state-changed (&rest _)
  "Send the state and the conversations followed to the devices shortly,
once changes settle."
  (when (and lispm-remote--sessions (not lispm-remote--state-timer))
    (setq lispm-remote--state-timer
          (run-at-time 0.3 nil (lambda ()
                                 (setq lispm-remote--state-timer nil)
                                 (when lispm-remote--machine
                                   (lispm-remote--send-state)
                                   (lispm-remote--send-chats)))))))

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
       (lispm-remote--send-message peer channel (lispm-remote--answer peer channel type message)))
      ("watch"
       (lispm-remote--send-message peer channel (lispm-remote--do peer channel "watch" message))
       (lispm-remote--send-chats))
      ('nil (lispm-remote--log "a message from %s is not a JSON object" peer))
      (_ (run-hook-with-args 'lispm-remote-message-functions peer channel message)))))

;;;; Commands

(defun lispm-remote--answer (peer channel type message)
  "Return the answer to the command MESSAGE of TYPE from PEER on CHANNEL,
doing it unless it repeats one already done."
  (let* ((id (alist-get 'id message))
         (key (and id (cons peer id))))
    (or (and key (cdr (assoc key lispm-remote--answers)))
        (let ((answer (lispm-remote--do peer channel type message)))
          (when key
            (setq lispm-remote--answers
                  (seq-take (cons (cons key answer) lispm-remote--answers)
                            lispm-remote--answers-kept)))
          answer))))

(defun lispm-remote--do (peer channel type message)
  "Do the command MESSAGE of TYPE from PEER on CHANNEL, and return its
answer, as JSON."
  (let ((id (and (alist-get 'id message) `((id . ,(alist-get 'id message))))))
    (condition-case error
        (progn
          (pcase type
            ("say" (lispm-remote-say (alist-get 'agent message) (alist-get 'text message)))
            ("decide" (lispm-remote-decide (alist-get 'agent message) (alist-get 'call message)
                                           (alist-get 'decision message)
                                           (alist-get 'reason message)))
            ("interrupt" (lispm-remote-interrupt (alist-get 'agent message)))
            ("watch" (lispm-remote--watch peer channel (alist-get 'agent message))))
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

;;;; Conversations

(defconst lispm-remote--chat-bytes 48000
  "The most bytes of items one chat message holds; a record holds 65518.")

(defconst lispm-remote--chat-text-length 4000
  "The most characters of an item's text a device is sent.")

(defconst lispm-remote--chat-result-length 600
  "The most characters of a call's result a device is sent.")

(defun lispm-remote--watch (peer channel name)
  "Have the session CHANNEL of PEER follow the conversation of LispM's
agent NAME, a string, or none when NAME is nil; send it again in full."
  (when name
    (lispm-remote--lispm-agent name))
  (when-let* ((session (lispm-remote--session peer channel)))
    (setf (lispm-remote--session-watch session) name
          (lispm-remote--session-chat session) nil)))

(defun lispm-remote--clip (text length)
  "Return TEXT, cut to LENGTH characters with an ellipsis if longer."
  (if (> (length text) length)
      (concat (substring text 0 (1- length)) "…")
    text))

(defun lispm-remote--chat-item (item)
  "Return ITEM of a conversation, as `lispm-chat-items' gives it, as JSON."
  (let ((text (plist-get item :text))
        (result (plist-get item :result)))
    `((kind . ,(format "%s" (plist-get item :kind)))
      ,@(and (stringp text)
             `((text . ,(lispm-remote--clip (substring-no-properties text)
                                            lispm-remote--chat-text-length))))
      ,@(and (plist-get item :from) `((from . ,(format "%s" (plist-get item :from)))))
      ,@(and (plist-get item :name) `((name . ,(format "%s" (plist-get item :name)))))
      ,@(and (plist-get item :state) `((state . ,(format "%s" (plist-get item :state)))))
      ,@(and (plist-member item :input)
             `((summary . ,(lispm-remote--summary (plist-get item :input)))))
      ,@(and (stringp result)
             `((result . ,(lispm-remote--clip (substring-no-properties result)
                                              lispm-remote--chat-result-length))))
      ,@(and (plist-get item :failed) '((failed . t))))))

(defun lispm-remote--chat (name)
  "Return the conversation of LispM's agent NAME as a chat message: its
latest items that fit one message."
  (let* ((agent (and (fboundp 'lispm-get-agent) (lispm-get-agent name t)))
         (buffer (and agent (lispm-agent-buffer agent)))
         (items (and (buffer-live-p buffer)
                     (or (fboundp 'lispm-chat-items) (require 'lispm-chat nil t))
                     (mapcar #'lispm-remote--chat-item (lispm-chat-items buffer))))
         (kept nil)
         (bytes 0))
    (cl-loop for item in (reverse items)
             for size = (string-bytes (json-serialize item))
             while (and (< (length kept) lispm-remote-chat-items)
                        (<= (+ bytes size) lispm-remote--chat-bytes))
             do (push item kept) (cl-incf bytes size))
    `((type . "chat") (agent . ,name) (items . ,(vconcat kept))
      (more . ,(- (length items) (length kept))))))

;;;; Pairing

(defun lispm-remote--require-mode ()
  "Turn on `lispm-remote-mode' unless it is on."
  (unless lispm-remote-mode
    (lispm-remote-mode 1)))

;;;###autoload
(defun lispm-remote-pair ()
  "Show a QR code a device pairs with LispM by."
  (interactive)
  (lispm-remote--require-mode)
  (setq lispm-remote--view 'pair)
  (lispm-remote--apply (lispm-remote-core-responder-offer lispm-remote--machine
                                                          lispm-remote-offer-seconds))
  (lispm-remote--check-peers)
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
  "Stop the pairing code working, and go back to LispM Remote's home."
  (interactive)
  (when (and lispm-remote--machine lispm-remote--offer
             (not (plist-get lispm-remote--offer :ended))
             (not (plist-get lispm-remote--offer :paired)))
    (lispm-remote--apply (lispm-remote-core-responder-withdraw lispm-remote--machine)))
  (setq lispm-remote--view 'home)
  (lispm-remote--render))

(defvar-keymap lispm-remote-pair-mode-map
  :doc "Keys of the pairing buffer."
  "a" #'lispm-remote-approve
  "r" #'lispm-remote-refuse
  "g" #'lispm-remote-pair
  "q" #'lispm-remote-withdraw)

(define-derived-mode lispm-remote-pair-mode special-mode "LispM Remote"
  "Mode of the buffer a device pairs with LispM in."
  (setq-local cursor-type nil
              left-margin-width 2)
  (add-hook 'kill-buffer-hook #'lispm-remote--stop-ticking nil t))

(defun lispm-remote--qr-rows ()
  "Return the rows of the offer's QR code, strings of 1 and 0, with a
quiet zone of four modules."
  (let* ((rows (lispm-remote-core-qr (concat lispm-remote-page "#"
                                             (plist-get lispm-remote--offer :token))))
         (blank (make-string (+ 8 (length (car rows))) ?0))
         (quiet (make-list 4 blank)))
    (append quiet (mapcar (lambda (row) (concat "0000" row "0000")) rows) quiet)))

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

;;;; The pairing screen

;; The screen walks through the steps both screens show, the page's too:
;;
;;   Join -- Scan -- Reach -- Confirm -- Connected
;;
;; A graphical Emacs draws the steps, the code, the countdown and the
;; fingerprint as SVG; a terminal gets the same in characters.  Each image
;; carries its text, so the buffer reads the same either way.  While the
;; screen waits on something it is redrawn every second; the QR code is
;; drawn once per code, and the images a redraw replaces are flushed.

(defface lispm-remote-title '((t :inherit bold :height 1.3))
  "The title of the pairing screen."
  :group 'lispm-remote)

(defface lispm-remote-done '((t :inherit success))
  "A step done."
  :group 'lispm-remote)

(defface lispm-remote-active '((t :inherit (bold link) :underline nil))
  "The step under way."
  :group 'lispm-remote)

(defface lispm-remote-pending '((t :inherit shadow))
  "A step to come."
  :group 'lispm-remote)

(defface lispm-remote-failed '((t :inherit error))
  "A step that failed."
  :group 'lispm-remote)

(defface lispm-remote-fingerprint '((t :inherit (bold fixed-pitch) :height 2.0))
  "The fingerprint the owner compares."
  :group 'lispm-remote)

(defface lispm-remote-button '((t :inherit custom-button))
  "The buttons of the pairing screen."
  :group 'lispm-remote)

(defconst lispm-remote--steps
  '((join . "Join") (scan . "Scan") (reach . "Reach") (confirm . "Confirm")
    (connected . "Connected"))
  "The steps of pairing, as both screens show them.")

(defvar-local lispm-remote--ticker nil
  "The timer redrawing the pairing screen while it waits.")

(defvar-local lispm-remote--images nil
  "The images the last drawing of the pairing screen made, but the code.")

(defun lispm-remote--progress (offer)
  "Return how far pairing through OFFER went: an alist of each step and
its state, `done', `active', `pending' or `failed'."
  (let* ((ended (plist-get offer :ended))
         (claimed (or (plist-get offer :fingerprint) (plist-get offer :paired)))
         (joined (or claimed (and lispm-remote--peers (> lispm-remote--peers 0))))
         (states `((join . ,(cond (joined 'done) ((lispm-remote--stranded-p) 'failed) (t 'active)))
                   (scan . ,(cond (claimed 'done) ((plist-get offer :token) 'active) (t 'pending)))
                   (reach . ,(if claimed 'done 'pending))
                   (confirm . ,(cond ((plist-get offer :paired) 'done) (claimed 'active) (t 'pending)))
                   (connected . ,(cond ((plist-get offer :opened) 'done)
                                       ((plist-get offer :paired) 'active)
                                       (t 'pending))))))
    (if (not ended)
        states
      (let ((at (if claimed 'confirm 'scan)))
        (mapcar (lambda (step)
                  (cond ((eq (car step) at) (cons at 'failed))
                        ((eq (cdr step) 'active) (cons (car step) 'pending))
                        (t step)))
                states)))))

(defun lispm-remote--waiting-p (offer)
  "Return non-nil when the screen for OFFER waits on something that moves."
  (and (plist-get offer :token)
       (not (plist-get offer :ended))
       (not (plist-get offer :opened))))

(defun lispm-remote--seconds-left (offer)
  "Return the seconds OFFER still works for."
  (max 0 (- (or (plist-get offer :expires) 0) (float-time))))

(defun lispm-remote--color (face fallback)
  "Return FACE's foreground, or FALLBACK where it has none, as in batch."
  (let ((color (face-foreground face nil t)))
    (if (and (stringp color) (not (string-prefix-p "unspecified" color))
             (color-defined-p color))
        (apply #'format "#%02x%02x%02x"
               (mapcar (lambda (value) (/ value 257)) (color-values color)))
      fallback)))

(defun lispm-remote--palette ()
  "Return the colors of the screen, from the faces: a plist."
  (list :done (lispm-remote--color 'lispm-remote-done "#2e9d4f")
        :active (lispm-remote--color 'lispm-remote-active "#2f6fde")
        :pending (lispm-remote--color 'lispm-remote-pending "#9a9a9a")
        :failed (lispm-remote--color 'lispm-remote-failed "#d23b3b")
        :text (lispm-remote--color 'default "#222222")))

(defun lispm-remote--graphic-p ()
  "Return non-nil when the screen is drawn with SVG images."
  (and (display-graphic-p) (image-type-available-p 'svg)))

(defun lispm-remote--flush (image)
  "Free IMAGE's cached pixels, in an Emacs built with images."
  (when (fboundp 'image-flush)
    (image-flush image)))

(defun lispm-remote--insert-image (image text)
  "Insert IMAGE, showing TEXT in its place where images are not shown."
  (push image lispm-remote--images)
  (insert-image image text))

;;;;; The steps

(defun lispm-remote--steps-text (progress)
  "Return PROGRESS as a line of characters."
  (mapconcat
   (lambda (step)
     (let ((label (alist-get (car step) lispm-remote--steps)))
       (pcase (cdr step)
         ('done (propertize (concat "✔ " label) 'face 'lispm-remote-done))
         ('active (propertize (concat "◉ " label) 'face 'lispm-remote-active))
         ('failed (propertize (concat "✖ " label) 'face 'lispm-remote-failed))
         (_ (propertize (concat "○ " label) 'face 'lispm-remote-pending)))))
   progress (propertize " ── " 'face 'lispm-remote-pending)))

(defun lispm-remote--steps-svg (progress pulse)
  "Return PROGRESS drawn as a row of steps; PULSE swells the active one."
  (let* ((palette (lispm-remote--palette))
         (width 600) (height 66) (left 36)
         (gap (/ (- width (* 2 left)) (1- (length progress))))
         (svg (svg-create width height)))
    (cl-loop for (_ . state) in progress for x from left by gap
             repeat (1- (length progress))
             do (svg-line svg x 22 (+ x gap) 22 :stroke-width 3 :stroke-linecap "round"
                          :stroke (plist-get palette (if (eq state 'done) :done :pending))))
    (cl-loop for (step . state) in progress for x from left by gap
             for color = (plist-get palette (pcase state ('done :done) ('active :active)
                                                  ('failed :failed) (_ :pending)))
             do (pcase state
                  ('done
                   (svg-circle svg x 22 13 :fill color)
                   (svg-polyline svg (list (cons (- x 6) 22) (cons (- x 1) 27) (cons (+ x 7) 17))
                                 :fill "none" :stroke "#ffffff" :stroke-width 3
                                 :stroke-linecap "round" :stroke-linejoin "round"))
                  ('active
                   (svg-circle svg x 22 (if pulse 16 13) :fill color :fill-opacity 0.15)
                   (svg-circle svg x 22 13 :fill "none" :stroke color :stroke-width 3)
                   (svg-circle svg x 22 5 :fill color))
                  ('failed
                   (svg-circle svg x 22 13 :fill color)
                   (svg-line svg (- x 5) 17 (+ x 5) 27 :stroke "#ffffff" :stroke-width 3
                             :stroke-linecap "round")
                   (svg-line svg (+ x 5) 17 (- x 5) 27 :stroke "#ffffff" :stroke-width 3
                             :stroke-linecap "round"))
                  (_ (svg-circle svg x 22 11 :fill "none" :stroke color :stroke-width 2)))
             (svg-text svg (alist-get step lispm-remote--steps)
                       :x x :y 58 :text-anchor "middle" :font-size 13
                       :font-family "sans-serif" :fill color
                       :font-weight (if (eq state 'active) "bold" "normal")))
    svg))

(defun lispm-remote--insert-steps (progress)
  "Insert PROGRESS, drawn or in characters."
  (let ((text (lispm-remote--steps-text progress)))
    (if (lispm-remote--graphic-p)
        (lispm-remote--insert-image
         (svg-image (lispm-remote--steps-svg progress (cl-evenp (truncate (float-time))))
                    :ascent 'center)
         text)
      (insert text))
    (insert "\n")))

;;;;; The countdown

(defun lispm-remote--bar-text (fraction width)
  "Return a bar WIDTH characters long, FRACTION of it filled."
  (let* ((eighths (round (* 8 width (min 1 (max 0 fraction)))))
         (full (/ eighths 8))
         (part (% eighths 8)))
    (concat (propertize (concat (make-string full ?█)
                                (if (> part 0) (string (aref " ▏▎▍▌▋▊▉" part)) ""))
                        'face (if (< fraction 0.2) 'lispm-remote-failed 'lispm-remote-active))
            (propertize (make-string (max 0 (- width full (if (> part 0) 1 0))) ?─)
                        'face 'lispm-remote-pending))))

(defun lispm-remote--bar-svg (fraction width)
  "Return a rounded bar WIDTH pixels long, FRACTION of it filled."
  (let* ((palette (lispm-remote--palette))
         (svg (svg-create width 10)))
    (svg-rectangle svg 0 0 width 10 :rx 5 :fill (plist-get palette :pending) :fill-opacity 0.3)
    (svg-rectangle svg 0 0 (max 10 (* width (min 1 (max 0 fraction)))) 10 :rx 5
                   :fill (plist-get palette (if (< fraction 0.2) :failed :active)))
    svg))

(defun lispm-remote--insert-countdown (offer)
  "Insert how long OFFER still works, as a bar and in minutes."
  (let* ((left (lispm-remote--seconds-left offer))
         (fraction (/ left (float (max 1 lispm-remote-offer-seconds))))
         (label (format "  %d:%02d left" (/ (truncate left) 60) (% (truncate left) 60))))
    (if (lispm-remote--graphic-p)
        (lispm-remote--insert-image (svg-image (lispm-remote--bar-svg fraction 300) :ascent 'center)
                                    (lispm-remote--bar-text fraction 30))
      (insert (lispm-remote--bar-text fraction 30)))
    (insert (propertize label 'face (if (< fraction 0.2) 'lispm-remote-failed 'lispm-remote-pending))
            "\n")))

;;;;; The code

(defun lispm-remote--qr-svg (rows size)
  "Return ROWS as a QR code SIZE pixels a side, in a frame whose corners
take the active color."
  (let ((modules (length rows))
        (accent (plist-get (lispm-remote--palette) :active)))
    (with-temp-buffer
      (insert (format "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" viewBox=\"-1 -1 %d %d\">"
                      size size (+ modules 2) (+ modules 2))
              (format "<rect x=\"-1\" y=\"-1\" width=\"%d\" height=\"%d\" rx=\"2\" fill=\"#ffffff\"/>"
                      (+ modules 2) (+ modules 2))
              "<path shape-rendering=\"crispEdges\" fill=\"#000000\" d=\"")
      (cl-loop for row in rows for y from 0
               do (cl-loop for module across row for x from 0
                           when (eq module ?1)
                           do (insert (format "M%d %dh1v1h-1z" x y))))
      (insert "\"/>")
      (let ((far (+ modules 0.5)) (arm 4))
        ;; One L at each corner, in the margin outside the quiet zone.
        (dolist (corner `((-0.5 -0.5 1 1) (,far -0.5 -1 1) (-0.5 ,far 1 -1) (,far ,far -1 -1)))
          (pcase-let ((`(,x ,y ,dx ,dy) corner))
            (insert (format "<path d=\"M%s %sH%sV%s\" fill=\"none\" stroke=\"%s\" stroke-width=\"0.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
                            (+ x (* dx arm)) y x (+ y (* dy arm)) accent)))))
      (insert "</svg>")
      (buffer-string))))

(defun lispm-remote--insert-code (offer)
  "Insert OFFER's QR code: drawn once per code, or in half blocks."
  (let ((rows (or (plist-get offer :rows)
                  (let ((rows (lispm-remote--qr-rows)))
                    (setq lispm-remote--offer (plist-put lispm-remote--offer :rows rows))
                    rows))))
    (if (lispm-remote--graphic-p)
        (let ((image (or (plist-get lispm-remote--offer :image)
                         (let ((image (create-image (lispm-remote--qr-svg rows 340) 'svg t
                                                    :ascent 'center)))
                           (setq lispm-remote--offer
                                 (plist-put lispm-remote--offer :image image))
                           image))))
          (insert "  ")
          (insert-image image "[QR code]"))
      (insert (lispm-remote--qr-text rows)))
    (insert "\n")))

;;;;; The fingerprint and the outcome

(defun lispm-remote--fingerprint-svg (fingerprint)
  "Return FINGERPRINT drawn as two groups of four, in boxes."
  (let* ((palette (lispm-remote--palette))
         (accent (plist-get palette :active))
         (svg (svg-create 300 64)))
    (cl-loop for group in (split-string fingerprint "-") for x from 0 by 156
             do (svg-rectangle svg (+ x 2) 2 140 60 :rx 10 :fill accent :fill-opacity 0.08
                               :stroke accent :stroke-width 2)
             (svg-text svg group :x (+ x 72) :y 44 :text-anchor "middle" :font-size 34
                       :font-family "monospace" :font-weight "bold" :letter-spacing 4
                       :fill (plist-get palette :text)))
    svg))

(defun lispm-remote--mark-svg (kind)
  "Return a large round mark: a check for KIND `done', a cross for `failed'."
  (let* ((palette (lispm-remote--palette))
         (svg (svg-create 72 72)))
    (svg-circle svg 36 36 32 :fill (plist-get palette (if (eq kind 'done) :done :failed)))
    (if (eq kind 'done)
        (svg-polyline svg '((21 . 37) (31 . 47) (52 . 26)) :fill "none" :stroke "#ffffff"
                      :stroke-width 6 :stroke-linecap "round" :stroke-linejoin "round")
      (svg-line svg 24 24 48 48 :stroke "#ffffff" :stroke-width 6 :stroke-linecap "round")
      (svg-line svg 48 24 24 48 :stroke "#ffffff" :stroke-width 6 :stroke-linecap "round"))
    svg))

(defun lispm-remote--insert-mark (kind)
  "Insert a large mark of KIND, `done' or `failed', then a line."
  (if (lispm-remote--graphic-p)
      (lispm-remote--insert-image (svg-image (lispm-remote--mark-svg kind) :ascent 'center)
                                  (if (eq kind 'done) "✔" "✖"))
    (insert (propertize (if (eq kind 'done) "✔" "✖")
                        'face (if (eq kind 'done) 'lispm-remote-done 'lispm-remote-failed))))
  (insert "\n\n"))

(defun lispm-remote--button-svg (text primary)
  "Return a rounded button saying TEXT: filled when PRIMARY is t, red
and outlined when it is `danger', else outlined."
  (let* ((palette (lispm-remote--palette))
         (accent (plist-get palette (if (eq primary 'danger) :failed :active)))
         (width (+ 36 (* 9 (length text))))
         (svg (svg-create width 38)))
    (if (eq primary t)
        (svg-rectangle svg 1 1 (- width 2) 36 :rx 18 :fill accent)
      (svg-rectangle svg 1 1 (- width 2) 36 :rx 18 :fill accent :fill-opacity 0.06
                     :stroke accent :stroke-width 2))
    (svg-text svg text :x (/ width 2) :y 24 :text-anchor "middle" :font-size 15
              :font-family "sans-serif" :font-weight "bold"
              :fill (if (eq primary t) "#ffffff" accent))
    svg))

(defun lispm-remote--insert-button (label key command &optional style)
  "Insert a button LABEL running COMMAND, which KEY also runs.
STYLE is t for the main button, `danger' for a refusal, nil otherwise."
  (let ((text (format "%s (%s)" label key)))
    (if (lispm-remote--graphic-p)
        (let ((image (svg-image (lispm-remote--button-svg text style) :ascent 'center)))
          (push image lispm-remote--images)
          (insert-text-button text 'display image 'face nil 'follow-link t
                              'help-echo (format "Press %s" key)
                              'action (lambda (_) (call-interactively command))))
      (insert-text-button (concat " " text " ") 'face 'lispm-remote-button 'follow-link t
                          'action (lambda (_) (call-interactively command)))))
  (insert "  "))

(defun lispm-remote--network-line ()
  "Return a line saying how the node stands on the network."
  (let ((seeds (mapconcat (lambda (seed) (string-remove-prefix "https://" seed))
                          (seq-remove #'lispm-remote--mismatched-p lispm-remote-seeds)
                          ", ")))
    (cond ((and lispm-remote--peers (> lispm-remote--peers 0))
           (propertize (format "On Rings, %d %s" lispm-remote--peers
                               (if (= lispm-remote--peers 1) "peer" "peers"))
                       'face 'lispm-remote-done))
          ((lispm-remote--stranded-p)
           (propertize "✖ Not on Rings: no seed runs this Rings version"
                       'face 'lispm-remote-failed))
          (t (propertize (format "%c Joining Rings%s"
                                 (aref "◐◓◑◒" (% (truncate (float-time)) 4))
                                 (if (string-empty-p seeds) "" (concat " through " seeds)))
                         'face 'lispm-remote-active)))))

(defun lispm-remote--insert-screen (offer)
  "Insert the pairing screen for OFFER."
  (let ((page (string-remove-suffix "/" (string-remove-prefix "https://" lispm-remote-page))))
    (insert (propertize "Pair a device with LispM" 'face 'lispm-remote-title) "\n\n")
    (lispm-remote--insert-steps (lispm-remote--progress offer))
    (insert "\n")
    (cond
     ((plist-get offer :paired)
      (lispm-remote--insert-mark 'done)
      (insert (propertize (format "Paired with %s." (plist-get offer :paired)) 'face 'bold) "\n\n"
              (if (plist-get offer :opened)
                  (propertize "✔ Session open, LispM's state sent" 'face 'lispm-remote-done)
                (propertize (format "%c Waiting for the device to open a session"
                                    (aref "◐◓◑◒" (% (truncate (float-time)) 4)))
                            'face 'lispm-remote-active))
              "\n\nIt can reach LispM whenever Emacs runs.\n"))
     ((plist-get offer :ended)
      (lispm-remote--insert-mark 'failed)
      (insert (pcase (plist-get offer :ended)
                ('expired "The code expired.")
                ('refused "You refused the device.")
                ('contested "Two devices used the code, so it stopped working: someone else may have seen it.")
                ('withdrawn "You withdrew the code.")
                ('replaced "A newer code replaced this one.")
                (_ "The code stopped working."))
              "\n\n")
      (lispm-remote--insert-button "New code" "g" #'lispm-remote-pair t)
      (insert "\n"))
     ((plist-get offer :fingerprint)
      (insert (propertize (format "%s asks to pair." (plist-get offer :name)) 'face 'bold)
              "\n\nIts screen shows\n\n  ")
      (let ((fingerprint (plist-get offer :fingerprint)))
        (if (lispm-remote--graphic-p)
            (lispm-remote--insert-image (svg-image (lispm-remote--fingerprint-svg fingerprint)
                                                   :ascent 'center)
                                        fingerprint)
          (insert (propertize fingerprint 'face 'lispm-remote-fingerprint))))
      (insert "\n\nIf the device shows the same, approve it.\n\n")
      (lispm-remote--insert-button "Approve" "a" #'lispm-remote-approve t)
      (lispm-remote--insert-button "Refuse" "r" #'lispm-remote-refuse 'danger)
      (insert "\n\n")
      (lispm-remote--insert-countdown offer))
     ((plist-get offer :token)
      (lispm-remote--insert-code offer)
      (insert (format "\nOpen %s on your phone or computer and scan this code.\nIt works once.\n\n"
                      (propertize page 'face 'bold)))
      (lispm-remote--insert-countdown offer)
      (insert "\n" (lispm-remote--network-line) "\n")
      (lispm-remote--insert-versions))
     (t
      (insert "No code yet.\n\n")
      (lispm-remote--insert-button "New code" "g" #'lispm-remote-pair t)
      (insert "\n")))))

(defun lispm-remote--render ()
  "Show the state of pairing in its buffer, and return the buffer."
  (let ((buffer (get-buffer-create lispm-remote--buffer))
        (offer lispm-remote--offer))
    (with-current-buffer buffer
      (let ((mode (if (eq lispm-remote--view 'pair) #'lispm-remote-pair-mode
                    #'lispm-remote-home-mode)))
        (unless (eq major-mode mode)
          ;; A new major mode drops the buffer's locals, the timer's too.
          (lispm-remote--stop-ticking)
          (let ((replaced lispm-remote--images))
            (funcall mode)
            (mapc #'lispm-remote--flush replaced))))
      (let ((inhibit-read-only t)
            (replaced lispm-remote--images))
        (setq lispm-remote--images nil)
        (erase-buffer)
        (if (eq lispm-remote--view 'pair)
            (lispm-remote--insert-screen offer)
          (lispm-remote--insert-home))
        (mapc #'lispm-remote--flush replaced)
        (goto-char (point-min)))
      (lispm-remote--keep-ticking (and (eq lispm-remote--view 'pair) offer)))
    buffer))

(defun lispm-remote--keep-ticking (offer)
  "Redraw the screen every second while OFFER's screen waits, and stop
when it no longer does."
  (cond ((and (lispm-remote--waiting-p offer) (not lispm-remote--ticker))
         (let ((buffer (current-buffer))
               (timer (timer-create)))
           (timer-set-time timer (time-add nil 1) 1)
           (timer-set-function timer #'lispm-remote--tick (list buffer timer))
           (timer-activate timer)
           (setq lispm-remote--ticker timer)))
        ((and (not (lispm-remote--waiting-p offer)) lispm-remote--ticker)
         (lispm-remote--stop-ticking))))

(defun lispm-remote--stop-ticking ()
  "Stop redrawing the pairing screen every second."
  (when lispm-remote--ticker
    (cancel-timer lispm-remote--ticker)
    (setq lispm-remote--ticker nil)))

(defun lispm-remote--tick (buffer timer)
  "Redraw BUFFER's pairing screen if it shows; cancel TIMER, which calls
this, once BUFFER is gone."
  (if (not (buffer-live-p buffer))
      (cancel-timer timer)
    (with-current-buffer buffer
      (cond ((not (lispm-remote--waiting-p lispm-remote--offer))
             (lispm-remote--stop-ticking))
            ((get-buffer-window buffer t)
             (lispm-remote--render))))))

(defun lispm-remote--refresh ()
  "Redraw LispM Remote's window if there is one."
  (when (get-buffer lispm-remote--buffer)
    (lispm-remote--render)))

;;;; Home

(defvar-keymap lispm-remote-home-mode-map
  :doc "Keys of LispM Remote's home."
  "p" #'lispm-remote-pair
  "d" #'lispm-remote-devices
  "t" #'lispm-remote-toggle
  "w" #'lispm-remote-show-windows
  "g" #'lispm-remote)

(define-derived-mode lispm-remote-home-mode special-mode "LispM Remote"
  "Mode of LispM Remote's home: how it stands, and the paired devices."
  (setq-local cursor-type nil
              left-margin-width 2)
  (add-hook 'kill-buffer-hook #'lispm-remote--stop-ticking nil t))

;;;###autoload
(defun lispm-remote ()
  "Open LispM Remote: whether devices can reach LispM, the paired
devices, and pairing another."
  (interactive)
  (setq lispm-remote--view 'home)
  (when lispm-remote-mode
    (lispm-remote--check-peers))
  (pop-to-buffer (lispm-remote--render)))

(defun lispm-remote-toggle ()
  "Turn `lispm-remote-mode' off when it is on, and on when it is off."
  (interactive)
  (lispm-remote-mode (if lispm-remote-mode -1 1))
  (lispm-remote--refresh))

(defun lispm-remote--local-display-p ()
  "Return non-nil when this checkout has the display's own page built,
for browsers on this computer's network."
  (and (require 'lispm-remote-display nil t)
       (fboundp 'lispm-remote-display-web-directory)
       (file-directory-p (expand-file-name "pkg" (lispm-remote-display-web-directory)))))

(declare-function lispm-remote-display-start "lispm-remote-display")
(declare-function lispm-remote-display-web-directory "lispm-remote-display")

(defun lispm-remote-show-windows ()
  "Serve LispM's windows to browsers on this computer's network, and copy
the page's URL.  Paired devices need none of this: they open LispM's
windows from their page."
  (interactive)
  (unless (lispm-remote--local-display-p)
    (user-error "This LispM Remote has no page for browsers on the network; paired devices open LispM's windows from lispm.ai/remote"))
  (call-interactively #'lispm-remote-display-start))

(defun lispm-remote--device-sessions (peer)
  "Return how many sessions the device PEER has open."
  (seq-count (lambda (session) (equal (lispm-remote--session-peer session) peer))
             lispm-remote--sessions))

(defun lispm-remote--insert-device (device)
  "Insert a line for the paired DEVICE, with a button forgetting it."
  (let* ((peer (plist-get device :peer))
         (sessions (lispm-remote--device-sessions peer)))
    (insert "  " (propertize (plist-get device :name) 'face 'bold)
            (propertize (format-time-string "  paired %F %R" (/ (plist-get device :paired-at) 1000.0))
                        'face 'lispm-remote-pending)
            "  "
            (if (> sessions 0)
                (propertize "● connected" 'face 'lispm-remote-done)
              (propertize "○ not connected" 'face 'lispm-remote-pending))
            "  ")
    (insert-text-button "Forget" 'face 'link 'follow-link t
                        'help-echo (format "Forget %s" (plist-get device :name))
                        'action (lambda (_)
                                  (when (yes-or-no-p (format "Forget %s? " (plist-get device :name)))
                                    (lispm-remote-forget peer))))
    (insert "\n")))

(defun lispm-remote--insert-home ()
  "Insert LispM Remote's home."
  (insert (propertize "LispM Remote" 'face 'lispm-remote-title) "\n\n")
  (if lispm-remote-mode
      (progn
        (insert (lispm-remote--network-line) "    ")
        (lispm-remote--insert-button "Turn off" "t" #'lispm-remote-toggle)
        (insert "\n" (propertize (format "This Emacs is %s on Rings"
                                          (lispm-remote-node-did lispm-remote--node))
                                  'face 'lispm-remote-pending)
                "\n")
        (lispm-remote--insert-versions)
        (delete-char -1))
    (insert (propertize "○ Off: no device can reach LispM" 'face 'lispm-remote-pending) "    ")
    (lispm-remote--insert-button "Turn on" "t" #'lispm-remote-toggle t))
  (insert "\n\n" (propertize "Paired devices" 'face 'bold) "\n\n")
  (if-let* ((devices (lispm-remote--devices)))
      (mapc #'lispm-remote--insert-device devices)
    (insert (propertize "  None yet." 'face 'lispm-remote-pending) "\n"))
  (insert "\n")
  (lispm-remote--insert-button "Pair a device" "p" #'lispm-remote-pair t)
  (when (lispm-remote--local-display-p)
    (lispm-remote--insert-button "Windows on this network" "w" #'lispm-remote-show-windows))
  (insert "\n\n"
          (propertize "A paired phone or computer opens lispm.ai/remote to talk to LispM's agents and\nwork in its windows, from anywhere, while this Emacs runs."
                      'face 'lispm-remote-pending)
          "\n"))

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

;; The module comes in the background, so that turning LispM Remote on
;; need not wait for it.
(unless noninteractive
  (run-with-idle-timer 1 nil #'lispm-remote-module-prefetch))

(provide 'lispm-remote)
;;; lispm-remote.el ends here
