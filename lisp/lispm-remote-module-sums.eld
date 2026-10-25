;;; lispm-remote-module-sums.eld --- The modules LispM Remote downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as
;; published, compressed: the module lispm-remote-module.el downloads for
;; these sources.  A module whose SHA-256 is not the one here is refused.
;; VERSION must be the version of the module's sources.
;; `lispm-remote-module-batch-pin' writes it once the module workflow has
;; published that release.

(:version "1ae3b7c1a87edf9a"
 :modules (("aarch64-apple-darwin" . "dd44d136486fb80b4a7bba1dfe29ad5767df3bb8ff203c35734c32fdedc0b335")
           ("aarch64-unknown-linux-gnu" . "f918154d2809890659e80ec6cd13e4964874f168c9d9b961ffe78467b5408048")
           ("x86_64-apple-darwin" . "30c32d040f9ce34cc7ab56d0e5aa498ffb91ddc4f0005801cd0b592e6f0f8bfe")
           ("x86_64-unknown-linux-gnu" . "8b306377098b9e62ae3e7363b8f6c0a7ccf453beba9d0e72bff970cab351e283")))
