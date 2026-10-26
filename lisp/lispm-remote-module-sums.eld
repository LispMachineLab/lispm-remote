;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "7aa6068aeee1637d"
 :modules (("aarch64-apple-darwin" . "d1ced90d791155fcfed8f73353c6341a0ff2b7e2348e710f5937dfc8b5b8d906")
           ("aarch64-unknown-linux-gnu" . "ac098d4fc70db7a3afe6c6592df9bebad4497b8ac95d1f6edb1ca107bff14db6")
           ("x86_64-apple-darwin" . "f988fa4d3f458c9fcbbac558b6533ed38b123570e0322d433c3000f685b3ec67")
           ("x86_64-unknown-linux-gnu" . "ce2457869fcca69251fbd20584a4d699d6a760a0d6c76eb66e31f3330804133e")))
