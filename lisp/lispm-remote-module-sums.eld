;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "8efe12ed9c421db8"
 :modules (("aarch64-apple-darwin" . "e0314c335167d900f0c8eaeb9d18c8207ee50fdf95b4994f0a920735a1f9447e")
           ("aarch64-unknown-linux-gnu" . "b208d8a87df479958e4e49d01b8d1ad27517b96b7f89681528c9f128f98d9cb0")
           ("x86_64-apple-darwin" . "22a321443bd5f837c77d2c93db5f5f69b8c331257cf0a49ec9961b2934bab014")
           ("x86_64-unknown-linux-gnu" . "fa200afb0f2b4d1052afc90ef302f40b6432c209332b980ca5a9112df9dffa45")))
