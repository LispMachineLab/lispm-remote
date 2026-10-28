;;; lispm-remote-module-sums.eld --- The modules lispm-remote-module downloads  -*- lisp-data -*-
;; SPDX-License-Identifier: AGPL-3.0-or-later

;; The SHA-256 of each module of the release module-VERSION, as published: what
;; lispm-prebuilt downloads for these sources.  One whose SHA-256 is not
;; the one here is refused.  `lispm-prebuilt-batch-pin' writes it once
;; the workflow has published that release.

(:version "d079f9e5cc66ef36"
 :modules (("aarch64-apple-darwin" . "231f65a3fcaeed523040da6f538e471177c62279f27cc81830420a7d810c81fd")
           ("aarch64-unknown-linux-gnu" . "8543143f0cf58a45164506eb15025318d0ae49a66c4a1967c045f47e6f8f4b3e")
           ("x86_64-apple-darwin" . "2afc0431666e1663703d0b88933aac7ba6a9bbf728d29acc21fae3ca9aabe691")
           ("x86_64-unknown-linux-gnu" . "234aa417fbfad6f52d1e13f7064b9b19c02158728cacc18497d3be242cf4fd72")))
