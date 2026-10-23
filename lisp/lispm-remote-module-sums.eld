;;; lispm-remote-module-sums.eld --- The modules LispM Remote downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as
;; published, compressed: the module lispm-remote-module.el downloads for
;; these sources.  A module whose SHA-256 is not the one here is refused.
;; VERSION must be the version of the module's sources.
;; `lispm-remote-module-batch-pin' writes it once the module workflow has
;; published that release.

(:version "1da4558664da0b12"
 :modules (("aarch64-apple-darwin" . "ab4029356217b6aecb4003c77480ad53784a451e55a075f38207d869cb26f6e7")
           ("aarch64-unknown-linux-gnu" . "e5230a6dfca3135cbd5c70abe12f92f7a4e13ccb75d37493f433bbbe1b4b91b8")
           ("x86_64-apple-darwin" . "d32828b28894b351cfa5fc6121c1774547df672b7ca2c9e7a1534b2ff39cd9b2")
           ("x86_64-unknown-linux-gnu" . "fe73e2c6f17cf222819b0683ae5152d74b6b44de2ef7677a0e79118bad683b24")))
