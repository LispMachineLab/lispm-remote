# LispM Remote

Reach a running [LispM](https://lispm.ai) from your phone or another computer. LispM shows a QR
code; the device opens one web page and scans it. From then on, whenever LispM is running and
online, the device can see what its agents are doing and answer them: approve a call, send a
message, stop a turn.

There is no LispM server in between. Both ends join [Rings Network](https://github.com/RingsNetwork/rings),
a peer-to-peer network that addresses peers by DID and connects them over WebRTC, so neither needs
a public address or an open port. The nodes that relay a frame learn who talks to whom, and nothing
of what is said.

## How it works

- **Pairing, once.** The QR code holds LispM's address and static key, a channel and a one-time
  pre-shared key. The device answers with a Noise `IKpsk1` handshake, which proves it read the
  code. Both screens show the same fingerprint, and LispM pairs only when you approve that
  fingerprint. A second device using the same code cancels the offer.
- **Sessions, any time after.** The device opens a channel with Noise `KK`. Each end already knows
  the other's static key, so every session is mutually authenticated and has fresh keys.
- **Records.** Each message is sealed under its own nonce. The receiver accepts each nonce at most
  once, in any order, so lost, repeated and reordered frames do no harm.

The protocol is two pure state machines: LispM's `Responder` and the device's `Initiator`. Time,
randomness and frames are values passed in and out, so each step is a function of its state and
its input. That is what lets the tests cover every order of a pairing's events.

## Layout

| Path                          | What it is                                                         |
|-------------------------------|--------------------------------------------------------------------|
| `crates/protocol`             | The pairing and session state machines, native and WebAssembly     |
| `crates/module`               | The Emacs module: a Rings node on its own threads inside Emacs     |
| `crates/web`                  | The device's machine for its page, in WebAssembly                  |
| `crates/display`              | The display protocol: screens, their differences, the codec        |
| `crates/relay`                | Emacs tty frames as display surfaces, and the clients' sessions    |
| `crates/client`               | The display's client, a canvas in WebAssembly, and its page        |
| `lisp/lispm-remote-module.el` | Gets the module and loads it                                       |
| `lisp/lispm-remote-node.el`   | The node in Lisp: start, connect, send, requests, incoming frames  |
| `lisp/lispm-remote.el`        | LispM Remote: the QR code, approval, devices, state, commands      |
| `lisp/lispm-remote-display.el`| LispM's windows, opened by clients                                 |
| `web/`                        | The page, served at lispm.ai/remote                                |
| `test/e2e/`                   | The browser test: a camera scanning the code, a network cut        |

## Pairing a device

`M-x lispm-remote` opens LispM Remote's window. It shows whether devices can reach LispM, with
a button that turns it on or off (`t`), and the paired devices, each marked connected or not, with a
Forget button. From there, `p` pairs another device.

`M-x lispm-remote-pair` turns on `lispm-remote-mode` and shows the pairing screen. It walks through
the same steps as the device's page, **Join, Scan, Reach, Confirm, Connected**, so you can follow
one pairing on either screen.

- **Scan.** The screen shows a QR code, with a bar counting down the two minutes it works. Open
  lispm.ai/remote on the device and scan the code.
- **Confirm.** When the device asks, both screens show the same fingerprint. Approve it with `a`,
  refuse it with `r`, or click either button.
- **Connected.** The screen shows the device's session opening.

A graphical Emacs draws the screen with SVG, and a terminal shows the same in characters.
`M-x lispm-remote-devices` lists the paired devices; press `d` there to forget one.

While the mode is on, the node joins Rings through `lispm-remote-seeds` (by default
`node.rings.rs`). It joins again whenever it has lost every peer. When a paired device opens a session,
LispM sends it its state, `{"type": "state", "state": …}`, and sends it again whenever it changes. The
state is Emacs's version and LispM's agents, with what each is doing; `lispm-remote-state-function`
changes it.

What happens when the network drops:

- **Lost frames.** Rings may lose a frame between hops without saying so. While a device waits for an
  answer, it sends its handshake again every two seconds. LispM answers a repeated handshake with the
  answer it gave the first time, so both ends keep the same keys.
- **The device's state.** A device that has no state yet asks for it with `{"type": "state?"}`.
- **Message sequences.** Rings refuses a sender whose message sequences go back. A node that restarts
  with the same account must therefore keep its replay records: this Emacs keeps them in
  `~/.emacs.d/lispm-remote/rings/`, and the page keeps them in IndexedDB.
- **Coming back.** A node that comes back while its old connection still looks alive is refused until
  that connection times out, about five seconds. The device keeps offering until then.

## Talking to agents from another device

On the page, each agent is a chat, as with Grok or Meta AI: you write to it, its answer streams
in, its calls show as they run, and the calls it waits on come with buttons to allow or deny them.
The device follows one agent's conversation at a time:

```json
{"type": "watch", "agent": "coder"}
```

LispM then sends the latest items of that conversation, `{"type": "chat", "agent": "coder",
"items": […], "more": 12}`, and sends them again whenever the conversation changes, while a
reply streams in too. The items are LispM's own chat's: what you and others said, the agent's
replies and thoughts, its calls with their state and result, and notes. As many as fit one
message are sent; `more` says how many earlier ones stay in Emacs.

Each agent in the state lists the calls it waits on, with the call's id, its tool and a short
summary. The device answers them, and talks to the agents, with three commands:

```json
{"type": "say", "agent": "coder", "text": "Run the tests again"}
{"type": "decide", "agent": "coder", "call": "toolu_01", "decision": "allow"}
{"type": "interrupt", "agent": "coder"}
```

A decision is `allow`, `always` (allow calls like it for the rest of the Emacs session) or
`deny`, as the 1, 2 and 3 of LispM's approval prompt. LispM answers each command with
`{"type": "ack"}` or `{"type": "error", "message": …}`, carrying the command's `id` if it had one.
A command repeated with the same id is answered again, not done again, so the page sends a
command until it hears back. LispM Remote does not need LispM: without it, the state has no agents
and every command is answered with an error.

## The page

The page is one static page: a little TypeScript, Rings's browser node, the device's machine from
`crates/web`, and [jsQR](https://github.com/cozmo/jsQR) for the camera. It has a Rings account of
its own, a WebCrypto P-256 key that cannot be read out of the browser, and keeps it, its static
key and its pairing in IndexedDB, with the node's replay records beside them.

It reads the offer from the QR code through the camera, or from its own URL: a phone's camera app
opens `https://lispm.ai/remote/#TOKEN`, and the page takes the token and removes it from the URL.
Pairing shows as five steps, the same Emacs shows: **Join** Rings through the seed, with its retries;
**Scan** the code in a viewfinder, skipped when the page came from the code's link; **Reach** LispM,
counting the requests sent; **Confirm** the fingerprint, in large type, against a bar of the time the
code has left; **Connected**, once LispM's state arrives. Reconnecting after the network dropped, or
after a reload, shows Join, Reach and Connected, so you see what the page waits for. Each step's
element carries `data-step` and `data-state`, which the browser test follows. Once paired, it lists LispM's agents,
what each is doing and how many calls wait for you; with one agent, its chat opens at once. In a
chat, what you send shows at once, until the conversation holds it; Enter sends, Shift-Enter starts
a new line, and an input method's Enter stays its own.

When the network drops, the page joins Rings again through the seed every second, then opens a new
session with LispM, without scanning again. It takes these from its URL:

| Parameter     | What it is                                    | Default                          |
|---------------|-----------------------------------------------|----------------------------------|
| `?seed=URL`   | The seed node it joins Rings through          | `https://node.rings.rs`          |
| `?ice=LIST`   | ICE servers, as Rings writes them             | `stun://stun.l.google.com:19302` |
| `?name=TEXT`  | What LispM calls this device                  | the browser and system           |
| `?debug`      | The page's state as `window.lispmRemote`      | off                              |

A seed answers a page only if it lets the page's origin call it (CORS); `node.rings.rs` lets
only `https://rings.rs` do so for now.

```sh
make page    # build it into web/dist: wasm-pack, npm and Rust's wasm32 target
make serve   # serve web/dist at http://localhost:8000
```

The first `make page` builds Rings's browser node from its release tag, which takes some minutes.

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

You never have to build the module. LispM Remote downloads the one published for your computer
(macOS on Apple silicon or Intel, GNU/Linux on x86_64 or arm64), and keeps it only if its SHA-256
matches the one pinned in `lisp/lispm-remote-module-sums.eld`. That file comes from the repository's
signed history, not from the release, so a module changed after it was pinned is refused. It is
downloaded in the background once LispM Remote opens, so turning it on need not wait.
[lispm-prebuilt](https://github.com/LispMachineLab/lispm-prebuilt) does the downloading, in
[lispm-kernel](https://github.com/LispMachineLab/lispm-kernel) when it runs; LispM's App Store
installs it with LispM Remote, and the Makefile finds a checkout of it beside this one
(`LISPM_PREBUILT=../lispm-prebuilt`).

To build the module yourself instead, set `(setq lispm-remote-module-source 'build)` or run
`make module`.

**Publishing a module.** The Module release workflow builds the module for each computer from
`main` and publishes it as `module-VERSION`, where VERSION is a digest of the module's sources. Each
version is built and published once. After that, `make pin` writes the checksums into the sums file
for the release that ships it, and `make check-pins` checks that file against the published release.

## LispM's windows on another device

A browser on a phone or another computer can show LispM's windows and work in them. Nothing is
streamed as video: LispM sends what each cell holds and where each image goes, and the page redraws
that on a canvas with its own fonts and images.

```elisp
(lispm-remote-display-start)   ; returns the page's URL, token included
(lispm-remote-display-stop)
```

Each window a page opens is a tty frame of the running Emacs, made on a pseudo-terminal the module
holds, the way `emacsclient -t` makes one on its own terminal. Emacs does all the layout and nothing
in Emacs changes; the module reads what Emacs writes as an xterm would and keeps the page up to date.
It answers Emacs as a recent xterm, so Emacs's own xterm support turns on what the page understands:
the mouse, keys such as `C-;`, bracketed paste, the clipboard.

- **Resources stay on the client.** An image is named by the SHA-256 of its bytes. A screen only
  places images by name; the page draws them from its pool and asks for an image only the first time
  it sees it, keeping it in the browser's cache from then on. The images LispM draws on a terminal
  (Kitty's graphics protocol) become such names on the way.
- **State, not a stream.** The module keeps each window's screen and sends a page the difference from
  the screen the page has, so a slow page skips the states in between, and a page that loses track
  asks for the whole screen again.
- **Which windows.** `lispm-remote-display-windows-function` lists what a page may open (the buffers,
  by default); `lispm-remote-display-show-function` shows an opened one in its frame.

For now pages connect over a WebSocket on the local network, with the token in the URL; the display
refuses a page without it, and pages from other sites. Rings, with the pairing above, is next. The page
is built with `make display-page`, which needs the `wasm-bindgen` command of the version
`crates/client` pins.

What a terminal cannot show, the page does not either: text is in cells, not proportional fonts; images
inside buffers and WebKit views are not drawn yet.

## Build and test

```sh
make module        # the module, for this computer
make test          # the Rust tests, then the Lisp tests on the built module
make compile       # byte-compile the Lisp
make page          # the page
make display-page  # the display's development page: the client in WebAssembly
make e2e           # the browser test, on Linux, with sudo
cargo check -p lispm-remote-protocol --target wasm32-unknown-unknown
```

The tests start Rings nodes on this computer and connect them over WebRTC by passing the offer and
answer between them directly, so they need no seed node. The tests of the device's commands use
LispM when it is checked out beside this repository (`../lisp_machine`, or `$LISPM_LISP`, the
directory of its Lisp), with a scripted model, and stand-ins for its functions otherwise.

The browser test runs the page in Playwright's Chromium against an Emacs running LispM Remote
(`test/e2e/run.sh`). The camera is Chromium's fake one, playing the QR code Emacs writes. The page
scans it, both ends show the same fingerprint, Emacs approves it, and the device talks to an agent.
Then the browser's network goes down: Chromium runs in a Linux network namespace of its own, and
`ip link` cuts it. LispM's state changes meanwhile; once the link is back, the page joins Rings again
and shows the new state, without scanning, and once more after a reload. A relay node in that Emacs
answers as the seed, under a name Chromium maps to it, since Rings refuses seeds at private
addresses.

## License

GNU Affero General Public License, version 3 or later. See [LICENSE](LICENSE).
