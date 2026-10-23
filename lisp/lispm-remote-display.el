;;; lispm-remote-display.el --- LispM's windows, redrawn on another device -*- lexical-binding: t; -*-

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

;; A client -- a page in a browser, on a phone or another computer --
;; shows LispM's windows by redrawing them: no video, only what each cell
;; holds and where each image goes, drawn with the client's own fonts and
;; from its own pool of images.
;;
;; Each window a client opens is a tty frame of this Emacs, made on a
;; pseudo-terminal the module holds, the way `emacsclient -t' makes one
;; on its own terminal.  Emacs does all the layout, and nothing in Emacs
;; changes; the module reads what Emacs writes and keeps clients up to
;; date with it.
;;
;;   M-x lispm-remote-display-start    serve the display; show and copy its URL
;;   M-x lispm-remote-display-stop     close its frames and stop
;;
;; Which windows a client may open is up to
;; `lispm-remote-display-windows-function' (the buffers, by default), and
;; how an opened one is shown up to `lispm-remote-display-show-function'.

;;; Code:

(require 'cl-lib)
(require 'subr-x)
(require 'lispm-remote-module)

(declare-function lispm-remote-core-display-start "lispm-remote-core")
(declare-function lispm-remote-core-display-stop "lispm-remote-core")
(declare-function lispm-remote-core-display-live-p "lispm-remote-core")
(declare-function lispm-remote-core-display-url "lispm-remote-core")
(declare-function lispm-remote-core-display-open "lispm-remote-core")
(declare-function lispm-remote-core-display-close "lispm-remote-core")
(declare-function lispm-remote-core-display-windows "lispm-remote-core")
(declare-function lispm-remote-core-display-requests "lispm-remote-core")
(declare-function lispm-remote-core-display-text "lispm-remote-core")

(defcustom lispm-remote-display-address "127.0.0.1:0"
  "Where the display serves clients: an address and a port.
The default takes any free port, reachable from this computer only.  To
reach the display from a phone on the same network, give an address
that network sees, such as \"0.0.0.0:7681\"; the page's URL carries a
token, and the display refuses anyone without it."
  :type 'string
  :group 'lispm-remote)

(defcustom lispm-remote-display-terminal-type "xterm-256color"
  "The terminal type the display's frames are made with.
The display answers Emacs as a recent xterm does, so Emacs's own
xterm support turns on what the display understands."
  :type 'string
  :group 'lispm-remote)

(defvar lispm-remote-display-windows-function #'lispm-remote-display-buffers
  "Function returning the windows clients may open.
It takes no argument and returns a list of (NAME . TITLE): NAME is what
`lispm-remote-display-show-function' is given, TITLE what a person reads.")

(defvar lispm-remote-display-show-function #'lispm-remote-display-show-buffer
  "Function showing the window a client opened.
It is called with the new frame and the window's NAME.")

(defvar lispm-remote-display-open-functions nil
  "Abnormal hook run with each frame the display made, once it shows.")

(defvar lispm-remote-display--handle nil
  "The module's display, while it runs.")

(defvar lispm-remote-display--process nil
  "The pipe process the module wakes Emacs through.")

(defvar lispm-remote-display--frames nil
  "The display's frames, as an alist of (SURFACE . FRAME).")

(defvar lispm-remote-display--offer-timer nil
  "The timer that offers clients the windows anew.")

(defvar lispm-remote-display-clients 0
  "How many clients are connected.")

;;;; Windows

(defun lispm-remote-display-buffers ()
  "Return the buffers a person works in, as windows."
  (let (windows)
    (dolist (buffer (buffer-list))
      (let ((name (buffer-name buffer)))
        (unless (or (string-prefix-p " " name) (minibufferp buffer))
          (push (cons name name) windows))))
    (nreverse windows)))

(defun lispm-remote-display-show-buffer (frame name)
  "Show the buffer NAME alone in FRAME."
  (when-let* ((buffer (get-buffer name)))
    (with-selected-frame frame
      (delete-other-windows)
      (switch-to-buffer buffer))))

(defun lispm-remote-display--offer ()
  "Offer clients the windows `lispm-remote-display-windows-function' gives."
  (setq lispm-remote-display--offer-timer nil)
  (when (lispm-remote-display-live-p)
    (lispm-remote-core-display-windows
     lispm-remote-display--handle
     (vconcat (mapcan (lambda (window) (list (car window) (cdr window)))
                      (funcall lispm-remote-display-windows-function))))))

(defun lispm-remote-display--offer-soon ()
  "Offer the windows anew in a moment, once for a burst of changes."
  (unless lispm-remote-display--offer-timer
    (setq lispm-remote-display--offer-timer
          (run-at-time 0.5 nil #'lispm-remote-display--offer))))

;;;; Starting and stopping

(defun lispm-remote-display-live-p ()
  "Return non-nil when the display is serving."
  (and lispm-remote-display--handle
       (lispm-remote-core-display-live-p lispm-remote-display--handle)))

(defun lispm-remote-display-web-directory ()
  "Return the directory the client page is served from."
  (expand-file-name "web" lispm-remote-module-root))

(defun lispm-remote-display-url ()
  "Return the URL of the display's page, token included."
  (unless (lispm-remote-display-live-p)
    (user-error "LispM's display is not running"))
  (lispm-remote-core-display-url lispm-remote-display--handle))

(defun lispm-remote-display-start ()
  "Serve LispM's display, and return its URL.
Interactively, show the URL and copy it: opening it in a browser shows
LispM's windows there.  The URL carries the display's token, so give it
only to your own devices."
  (interactive)
  (unless (lispm-remote-display-live-p)
    (lispm-remote-module-load)
    ;; A pipe process makes a buffer named as it is; the space hides it.
    (let ((process (make-pipe-process
                    :name " lispm-remote-display" :noquery t :coding 'binary
                    :filter (lambda (_process _output) (lispm-remote-display--collect)))))
      (condition-case error
          (setq lispm-remote-display--handle
                (lispm-remote-core-display-start
                 lispm-remote-display-address (lispm-remote-display-web-directory) process))
        (error (delete-process process)
               (signal (car error) (cdr error))))
      (setq lispm-remote-display--process process)
      (add-hook 'buffer-list-update-hook #'lispm-remote-display--offer-soon)
      (add-hook 'delete-frame-functions #'lispm-remote-display--frame-deleted)
      (add-hook 'kill-emacs-hook #'lispm-remote-display-stop)
      (lispm-remote-display--offer)))
  (let ((url (lispm-remote-display-url)))
    (when (called-interactively-p 'interactive)
      (kill-new url)
      (message "LispM's display: %s (copied)" url))
    url))

(defun lispm-remote-display-stop ()
  "Delete the display's frames and stop serving."
  (interactive)
  (dolist (entry lispm-remote-display--frames)
    (when (frame-live-p (cdr entry))
      (let ((delete-frame-functions nil))
        (delete-frame (cdr entry) t))))
  (setq lispm-remote-display--frames nil)
  (when lispm-remote-display--handle
    ;; The module stops writing before its pipe goes: a write to a closed
    ;; pipe raises SIGPIPE, which ends a batch Emacs.
    (lispm-remote-core-display-stop lispm-remote-display--handle)
    (setq lispm-remote-display--handle nil))
  (when lispm-remote-display--process
    (delete-process lispm-remote-display--process)
    (setq lispm-remote-display--process nil))
  (when lispm-remote-display--offer-timer
    (cancel-timer lispm-remote-display--offer-timer)
    (setq lispm-remote-display--offer-timer nil))
  (remove-hook 'buffer-list-update-hook #'lispm-remote-display--offer-soon)
  (remove-hook 'delete-frame-functions #'lispm-remote-display--frame-deleted)
  (setq lispm-remote-display-clients 0))

;;;; Frames

(defun lispm-remote-display-open (window cols rows &optional cell-width cell-height)
  "Open WINDOW on a new surface of COLS by ROWS cells; return its frame.
CELL-WIDTH and CELL-HEIGHT are a client's cell in pixels, for images."
  (pcase-let* ((`(,surface . ,tty)
                (lispm-remote-core-display-open lispm-remote-display--handle window cols rows
                                                (or cell-width 8) (or cell-height 16)))
               (frame (condition-case error
                          (make-frame `((window-system . nil)
                                        (tty . ,tty)
                                        (tty-type . ,lispm-remote-display-terminal-type)
                                        (lispm-remote-display-surface . ,surface)))
                        (error (lispm-remote-core-display-close lispm-remote-display--handle surface)
                               (signal (car error) (cdr error))))))
    (push (cons surface frame) lispm-remote-display--frames)
    (funcall lispm-remote-display-show-function frame window)
    (run-hook-with-args 'lispm-remote-display-open-functions frame)
    frame))

(defun lispm-remote-display-frame-surface (frame)
  "Return the surface FRAME is the display's frame for, or nil."
  (frame-parameter frame 'lispm-remote-display-surface))

(defun lispm-remote-display-close (surface)
  "Close SURFACE: delete its frame, which closes the surface too."
  (let ((frame (alist-get surface lispm-remote-display--frames)))
    (if (frame-live-p frame)
        (delete-frame frame t)
      (lispm-remote-display--forget surface))))

(defun lispm-remote-display--frame-deleted (frame)
  "Close FRAME's surface once FRAME is gone, if it is the display's."
  (when-let* ((surface (lispm-remote-display-frame-surface frame)))
    ;; Emacs still writes to the frame's terminal while deleting it, so
    ;; the surface stays until that is done.
    (run-at-time 0 nil #'lispm-remote-display--forget surface)))

(defun lispm-remote-display--forget (surface)
  "Close SURFACE in the module and forget its frame."
  (setq lispm-remote-display--frames (assq-delete-all surface lispm-remote-display--frames))
  (when (lispm-remote-display-live-p)
    (lispm-remote-core-display-close lispm-remote-display--handle surface)))

;;;; Clients' requests

(defun lispm-remote-display--collect ()
  "Act on the requests clients left."
  (when (lispm-remote-display-live-p)
    (dolist (request (lispm-remote-core-display-requests lispm-remote-display--handle))
      ;; A frame is made from a timer, not inside the process filter:
      ;; making it asks the terminal questions and waits for the answers.
      (run-at-time 0 nil #'lispm-remote-display--act request))))

(defun lispm-remote-display--act (request)
  "Act on REQUEST, which a client left."
  (condition-case-unless-debug error
      (pcase request
        (`(open ,window ,cols ,rows ,cell-width ,cell-height)
         (lispm-remote-display-open window cols rows cell-width cell-height))
        (`(close ,surface) (lispm-remote-display-close surface))
        (`(clients ,count) (setq lispm-remote-display-clients count)))
    (error (message "LispM's display: %s" (error-message-string error)))))

(provide 'lispm-remote-display)
;;; lispm-remote-display.el ends here
