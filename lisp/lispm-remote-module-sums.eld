;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "2fdab13221e437f7"
 :modules (("aarch64-apple-darwin" . "863a0b6e3f38f95fd93225a2426c73049e878cb501e46dff7f355e90ce8f1048")
           ("aarch64-unknown-linux-gnu" . "9c49f771028210641c4f31c6dd26924e858f2ace9c22e0d21793e6dc6707119e")
           ("x86_64-apple-darwin" . "5b6d67567f9b68d59cd9c1a2866bbd3f910af5fa68365f43a1af63e12bd0960e")
           ("x86_64-unknown-linux-gnu" . "99f6b14bfd288d1b4810f61dbb49a38348919b6bdc041f2b79721a9591cb3940")))
