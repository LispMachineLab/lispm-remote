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

| Path              | What it is                                                     |
|-------------------|----------------------------------------------------------------|
| `crates/protocol` | The pairing and session state machines, native and WebAssembly |

To come: the Emacs module that runs a Rings node inside LispM, the phone's page, and
`lispm-remote.el`, which connects the channel to LispM's agents.

## Build and test

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo check -p lispm-remote-protocol --target wasm32-unknown-unknown
```

## License

GNU Affero General Public License, version 3 or later. See [LICENSE](LICENSE).
