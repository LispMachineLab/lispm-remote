# LispM Remote

Reach a running [LispM](https://lispm.ai) from your phone. LispM shows a QR code; the phone opens
one web page and scans it. From then on, whenever LispM is running and online, the phone can see
what its agents are doing and answer them: approve a call, send a message, stop a turn.

There is no LispM server in between. Both ends join [Rings Network](https://github.com/RingsNetwork/rings),
a peer-to-peer network that addresses peers by DID and connects them over WebRTC, so neither needs
a public address or an open port. The nodes that relay a frame learn who talks to whom, and nothing
of what is said.

## How it works

- **Pairing, once.** The QR code holds LispM's address and static key, a channel and a one-time
  pre-shared key. The phone answers with a Noise `IKpsk1` handshake, which proves it read the
  code. Both screens show the same fingerprint, and LispM pairs only when you approve that
  fingerprint. A second device using the same code cancels the offer.
- **Sessions, any time after.** The phone opens a channel with Noise `KK`. Each end already knows
  the other's static key, so every session is mutually authenticated and has fresh keys.
- **Records.** Each message is sealed under its own nonce. The receiver accepts each nonce at most
  once, in any order, so lost, repeated and reordered frames do no harm.

The protocol is two pure state machines: LispM's `Responder` and the phone's `Initiator`. Time,
randomness and frames are values passed in and out, so each step is a function of its state and
its input. That is what lets the tests cover every order of a pairing's events.

## Layout

| Path                          | What it is                                                         |
|-------------------------------|--------------------------------------------------------------------|
| `crates/protocol`             | The pairing and session state machines, native and WebAssembly     |
| `crates/module`               | The Emacs module: a Rings node on its own threads inside Emacs     |
| `lisp/lispm-remote-module.el` | Gets the module and loads it                                       |
| `lisp/lispm-remote-node.el`   | The node in Lisp: start, connect, send, requests, incoming frames  |
| `lisp/lispm-remote.el`        | LispM Remote: the QR code, approval, devices, state, reconnecting  |

To come: the phone's page at lispm.ai/remote, and the browser test that scans the QR code with a
camera feed and cuts the network.

## Pairing a phone

`M-x lispm-remote-pair` turns on `lispm-remote-mode` and shows a QR code: a picture in a graphical
Emacs, half blocks in a terminal. Open lispm.ai/remote on the phone and scan it. When the phone asks,
both screens show the same fingerprint. Press `a` to approve it or `r` to refuse it. The code works
once, for two minutes. `M-x lispm-remote-devices` lists the paired devices; press `d` there to forget
one.

While the mode is on, the node joins Rings through `lispm-remote-seeds` (by default
`node.rings.rs`). It joins again whenever it has lost every peer. When a paired phone opens a session,
LispM sends it its state, `{"type": "state", "state": …}`, and sends it again whenever it changes. The
state is Emacs's version and LispM's agents, with what each is doing; `lispm-remote-state-function`
changes it.

What happens when the network drops:

- **Lost frames.** Rings may lose a frame between hops without saying so. While a phone waits for an
  answer, it sends its handshake again every two seconds. LispM answers a repeated handshake with the
  answer it gave the first time, so both ends keep the same keys.
- **The phone's state.** A phone that has no state yet asks for it with `{"type": "state?"}`.
- **Message sequences.** Rings refuses a sender whose message sequences go back. A node that restarts
  with the same account must therefore keep its replay records: this Emacs keeps them in
  `~/.emacs.d/lispm-remote/rings/`, and the page keeps them in IndexedDB.
- **Coming back.** A node that comes back while its old connection still looks alive is refused until
  that connection times out, about five seconds. The phone keeps offering until then.

## The node in Emacs

The node runs on the module's own threads. Emacs starts work and later gets the outcome back
through callbacks, so it never waits on the network. Frames from other peers arrive in
`lispm-remote-node-frame-functions`, with the sender's DID as Rings authenticated it.

```elisp
(setq node (lispm-remote-node-start))
(lispm-remote-node-connect node "https://node.rings.rs")
(lispm-remote-node-send node did frame)            ; frame: a unibyte string
(lispm-remote-node-request node "listPeers" nil
                           (lambda (result error) ...))
```

The node's identity is a Rings account key. It is made the first time and kept in
`~/.emacs.d/lispm-remote/account`, which only you can read.

LispM's modules are meant to be downloaded and checked against pinned checksums. No module is
published yet, so for now you have to choose to build it: `(setq lispm-remote-module-source 'build)`,
or `make module`.

## Build and test

```sh
make module      # the module, for this computer
make test        # the Rust tests, then the Lisp tests on the built module
make compile     # byte-compile the Lisp
cargo check -p lispm-remote-protocol --target wasm32-unknown-unknown
```

The tests start Rings nodes on this computer and connect them over WebRTC by passing the offer and
answer between them directly, so they need no seed node.

## License

GNU Affero General Public License, version 3 or later. See [LICENSE](LICENSE).
