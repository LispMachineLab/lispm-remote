;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "03c7b4605818b06d"
 :modules (("aarch64-apple-darwin" . "d62b715a025ec057c4e63691889f27aa178af8ab342ed8336b9ba1e517e022bb")
           ("aarch64-unknown-linux-gnu" . "ecf1f15a09b0a075db63855cc05fcd244d5173b19793b5e81157185a3002b298")
           ("x86_64-apple-darwin" . "30d6d493c1c20e74f89b2d1a31b4a3527d4a46ff99a0ce6564d2b08fceb56968")
           ("x86_64-unknown-linux-gnu" . "f0705dc87a161e251626341497a00274731cb7c5df4c5504873c908b9279e55d")))
