;;; lispm-remote-display-test.el --- Tests of LispM's display -*- lexical-binding: t; -*-

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

;; This Emacs draws a buffer on a tty frame whose terminal the module
;; holds, and the module's copy of the screen shows what Emacs drew.
;; The module must be built (`make module'); without it, the tests are
;; skipped.

;;; Code:

(require 'ert)
(require 'lispm-remote-display)

(defconst lispm-remote-display-test--patience 30
  "Seconds a test waits for Emacs to draw.")

(defun lispm-remote-display-test--module-p ()
  "Return non-nil when the built module can be loaded."
  (let ((lispm-remote-module-source 'build))
    (and (ignore-errors (lispm-prebuilt-built-file 'lispm-remote-module))
         (file-readable-p (lispm-prebuilt-built-file 'lispm-remote-module))
         (progn (lispm-remote-module-load) t))))

(defun lispm-remote-display-test--wait (predicate)
  "Redisplay and process output until PREDICATE returns non-nil; return
its value.  Fail after `lispm-remote-display-test--patience' seconds."
  (let ((deadline (+ (float-time) lispm-remote-display-test--patience))
        value)
    (while (and (not (setq value (funcall predicate)))
                (< (float-time) deadline))
      (redisplay t)
      (accept-process-output nil 0.05))
    (or value (ert-fail "the display did not show it in time"))))

(defmacro lispm-remote-display-test--with-display (&rest body)
  "Run BODY with the display serving, and stop it afterwards."
  (declare (indent 0) (debug t))
  `(progn
     (skip-unless (lispm-remote-display-test--module-p))
     (unwind-protect
         (progn (lispm-remote-display-start) ,@body)
       (lispm-remote-display-stop))))

(defun lispm-remote-display-test--text (surface)
  "Return the text the module's copy of SURFACE shows."
  (lispm-remote-core-display-text lispm-remote-display--handle surface))

(ert-deftest lispm-remote-display-test-url-carries-a-token ()
  "The display's URL is local and carries a 128-bit token."
  (lispm-remote-display-test--with-display
    (should (string-match-p "\\`http://127\\.0\\.0\\.1:[0-9]+/#t=[0-9a-f]\\{32\\}\\'"
                            (lispm-remote-display-url)))))

(ert-deftest lispm-remote-display-test-a-buffer-is-redrawn-from-its-frame ()
  "A buffer opened on the display shows on its surface, and follows
the buffer's changes."
  (lispm-remote-display-test--with-display
    (let* ((buffer (get-buffer-create "lispm-remote-display-test"))
           (frame (progn
                    (with-current-buffer buffer
                      (erase-buffer)
                      (insert "drawn by Emacs, redrawn by the client"))
                    (lispm-remote-display-open (buffer-name buffer) 60 12)))
           (surface (lispm-remote-display-frame-surface frame)))
      (unwind-protect
          (progn
            (should (frame-live-p frame))
            (should (eq (window-buffer (frame-root-window frame)) buffer))
            (lispm-remote-display-test--wait
             (lambda ()
               (string-match-p "drawn by Emacs, redrawn by the client"
                               (lispm-remote-display-test--text surface))))
            (with-current-buffer buffer
              (goto-char (point-max))
              (insert "\nand changed"))
            (lispm-remote-display-test--wait
             (lambda ()
               (string-match-p "and changed" (lispm-remote-display-test--text surface)))))
        (kill-buffer buffer)))))

(ert-deftest lispm-remote-display-test-closing-deletes-the-frame ()
  "Closing a surface deletes its frame, and the surface goes."
  (lispm-remote-display-test--with-display
    (let* ((frame (lispm-remote-display-open "*scratch*" 40 8))
           (surface (lispm-remote-display-frame-surface frame)))
      (lispm-remote-display-close surface)
      (should-not (frame-live-p frame))
      (lispm-remote-display-test--wait
       (lambda () (null (lispm-remote-display-test--text surface))))
      (should-not (assq surface lispm-remote-display--frames)))))

(ert-deftest lispm-remote-display-test-drawn-while-only-the-initial-frame-was-selected ()
  "The display's frames redraw even when the initial frame was selected,
as in a daemon nobody visited: Emacs redisplays nothing then, so the
display keeps one of its own frames selected."
  (lispm-remote-display-test--with-display
    (skip-unless (lispm-remote-display--initial-p (selected-frame)))
    (let* ((initial (selected-frame))
           (buffer (get-buffer-create "lispm-remote-display-test-initial"))
           (kept (lispm-remote-display-open (buffer-name buffer) 40 8))
           (closed (lispm-remote-display-open "*scratch*" 40 8))
           (surface (lispm-remote-display-frame-surface kept)))
      (unwind-protect
          (progn
            (select-frame initial)
            (lispm-remote-display-close (lispm-remote-display-frame-surface closed))
            (lispm-remote-display-test--wait
             (lambda () (not (lispm-remote-display--initial-p (selected-frame)))))
            (with-current-buffer buffer
              (insert "drawn with the initial frame selected"))
            (lispm-remote-display-test--wait
             (lambda ()
               (string-match-p "drawn with the initial frame selected"
                               (lispm-remote-display-test--text surface)))))
        (kill-buffer buffer)))))

(ert-deftest lispm-remote-display-test-buffers-are-offered ()
  "The windows offered are the buffers a person works in."
  (let ((windows (lispm-remote-display-buffers)))
    (should (assoc "*scratch*" windows))
    (should-not (seq-some (lambda (window) (string-prefix-p " " (car window))) windows))))

(provide 'lispm-remote-display-test)
;;; lispm-remote-display-test.el ends here
