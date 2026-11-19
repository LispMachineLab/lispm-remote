;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "bcdbac4405170a62"
 :modules (("aarch64-apple-darwin" . "dbc175cf531e06504549469798db3ccabc4bff14b8877b48c5233391a2188777")
           ("aarch64-unknown-linux-gnu" . "f0c1f84a3927937e729c236a8c55e84a2b81755e0476909e84d0a23053d2e137")
           ("x86_64-apple-darwin" . "1d2bf3e19679cd7b9f61ba93bb77c6099343c1c27e6d42c856200ad2ba9040da")
           ("x86_64-unknown-linux-gnu" . "bb3b6ddb5e46eca8a001e3933522f5286b3ec236ffa9ed8681c120abb373f837")))
