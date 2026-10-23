;;; lispm.el --- LispM's side of the browser test -*- lexical-binding: t; -*-

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

;; The Emacs the browser test talks to, run by run.sh in batch:
;;
;; - a relay node, standing for the network, with the two public halves of
;;   Rings's HTTP handshake (nodeDid, answerOffer) on $E2E_HOST:$E2E_PORT,
;;   so the page joins it as it joins a seed;
;; - `lispm-remote-mode', its node connected to the relay by hand;
;; - one agent, "coder", standing in for LispM's: told something, it
;;   answers "Heard: " and what it was told.
;;
;; The test drives it through files in $E2E_DIR: each form written to
;; eval.el is evaluated, and its value, printed, written to eval.out.  It
;; writes `ready' once all is up.

;;; Code:

(require 'cl-lib)
(require 'lispm-remote)
(require 'lispm-remote-node-test)

(defvar e2e-dir (file-name-as-directory (getenv "E2E_DIR"))
  "The directory the test and this Emacs talk through.")

(setq lispm-remote-module-source 'build
      lispm-remote-directory (expand-file-name "lispm-remote/" e2e-dir)
      lispm-remote-seeds nil
      lispm-remote-check-seconds 2
      lispm-remote-offer-seconds 300)
(lispm-remote-module-load)

;;;; One agent, standing in for LispM's

(cl-defstruct (e2e-agent (:copier nil))
  "A stand-in for one of LispM's agents."
  (name "coder") (state 'idle) status items
  (buffer (generate-new-buffer " *coder*")))

(defvar e2e-coder (make-e2e-agent)
  "The agent the device talks to.")

(defvar lispm-state-change-functions nil)

(defun e2e-changed ()
  "Tell LispM Remote that the agent changed, as LispM does."
  (run-hook-with-args 'lispm-state-change-functions e2e-coder))

(defun e2e-status (status)
  "Have the agent say it is doing STATUS."
  (setf (e2e-agent-status e2e-coder) status)
  (e2e-changed)
  status)

(defun lispm-agents () (list e2e-coder))
(defun lispm-get-agent (name &optional _noerror)
  (and (equal (format "%s" name) "coder") e2e-coder))
(defun lispm-agent-name (agent) (e2e-agent-name agent))
(defun lispm-agent-role (_agent) 'assistant)
(defun lispm-agent-root (_agent) "~/project/")
(defun lispm-agent-state (agent) (e2e-agent-state agent))
(defun lispm-agent-status (agent) (e2e-agent-status agent))
(defun lispm-agent-buffer (agent) (e2e-agent-buffer agent))
(defun lispm-agent-pending (_agent) (make-hash-table :test #'equal))
(defun lispm-waiting-calls (_agent) nil)
(defun lispm-chat-items (_buffer) (reverse (e2e-agent-items e2e-coder)))

(defun lispm-tell (agent text &rest _)
  "Give AGENT the input TEXT; it answers a moment later."
  (push (list :kind 'user :text text) (e2e-agent-items agent))
  (setf (e2e-agent-state agent) 'requesting)
  (e2e-changed)
  (run-at-time 1 nil (lambda ()
                       (push (list :kind 'text :text (concat "Heard: " text))
                             (e2e-agent-items agent))
                       (setf (e2e-agent-state agent) 'idle)
                       (e2e-changed)))
  agent)

;;;; The network: a relay, and LispM's node connected to it

(defvar e2e-relay (lispm-remote-node-start (lispm-remote-core-generate-account))
  "The node standing for the network.")

(lispm-remote-mode 1)
(lispm-remote-node-test--connect lispm-remote--node e2e-relay)

;;;; The relay's HTTP endpoint, as a seed's

(defconst e2e-public-methods '("nodeDid" "answerOffer")
  "The methods a seed answers anyone: the two halves of the handshake.")

(defun e2e-http-reply (process status body)
  "Answer the HTTP request on PROCESS with STATUS and the JSON BODY."
  (when (process-live-p process)
    (process-send-string
     process
     (concat (format "HTTP/1.1 %d %s\r\n" status (if (= status 200) "OK" "No Content"))
             "Access-Control-Allow-Origin: *\r\n"
             "Access-Control-Allow-Methods: POST, OPTIONS\r\n"
             "Access-Control-Allow-Headers: authorization, content-type, accept\r\n"
             "Content-Type: application/json\r\n"
             (format "Content-Length: %d\r\nConnection: close\r\n\r\n" (string-bytes body))
             body))
    (run-at-time 2 nil (lambda () (when (process-live-p process) (delete-process process))))))

(defun e2e-http-answer (process request)
  "Answer the JSON-RPC REQUEST on PROCESS through the relay."
  (let ((id (alist-get 'id request))
        (method (alist-get 'method request)))
    (if (not (member method e2e-public-methods))
        (e2e-http-reply process 200 (json-serialize
                                     `((jsonrpc . "2.0") (id . ,id)
                                       (error (code . -32601) (message . "Method not found")))))
      (lispm-remote-node-request
       e2e-relay method (alist-get 'params request)
       (lambda (result error)
         (e2e-http-reply
          process 200
          (json-serialize
           (if error
               `((jsonrpc . "2.0") (id . ,id)
                 (error (code . -32000) (message . ,(format "%s" error))))
             `((jsonrpc . "2.0") (id . ,id) (result . ,(or result (make-hash-table))))))))))))

(defun e2e-http-filter (process chunk)
  "Gather the HTTP request on PROCESS from CHUNK, and answer it once whole."
  (let ((data (concat (or (process-get process 'data) "") chunk)))
    (process-put process 'data data)
    (when-let* ((end (string-search "\r\n\r\n" data)))
      (let* ((head (downcase (substring data 0 end)))
             (body (substring data (+ end 4)))
             (size (if (string-match "^content-length: *\\([0-9]+\\)" head)
                       (string-to-number (match-string 1 head))
                     0)))
        (when (>= (length body) size)
          (process-put process 'data nil)
          (if (string-prefix-p "options" head)
              (e2e-http-reply process 204 "")
            (condition-case nil
                (e2e-http-answer process (json-parse-string (decode-coding-string body 'utf-8)
                                                            :object-type 'alist))
              (json-error (e2e-http-reply process 200 "{}")))))))))

(make-network-process :name "e2e-seed" :server t :family 'ipv4
                      :host (getenv "E2E_HOST")
                      :service (string-to-number (getenv "E2E_PORT"))
                      :coding 'binary :filter #'e2e-http-filter)

;;;; Driven through files

(defun e2e-poll ()
  "Evaluate the form the test wrote, if any, and write its value."
  (let ((request (expand-file-name "eval.el" e2e-dir)))
    (when (file-exists-p request)
      (let* ((form (with-temp-buffer (insert-file-contents request) (buffer-string)))
             (value (progn
                      (delete-file request)
                      (save-current-buffer
                        (condition-case error
                            (prin1-to-string (eval (car (read-from-string form)) t))
                          (error (format "(error %S)" error)))))))
        (with-temp-file (expand-file-name "eval.tmp" e2e-dir)
          (insert value))
        (rename-file (expand-file-name "eval.tmp" e2e-dir)
                     (expand-file-name "eval.out" e2e-dir) t)))))

(run-at-time 0.2 0.2 #'e2e-poll)
(with-temp-file (expand-file-name "ready" e2e-dir)
  (insert (lispm-remote-node-did lispm-remote--node)))
(message "e2e: LispM is up")
(while t
  (accept-process-output nil 0.2))

;;; lispm.el ends here
