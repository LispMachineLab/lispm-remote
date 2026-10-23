// LispM Remote: the display's development page.  It reaches the display
// over a WebSocket on the local network, with the token LispM's URL
// carries after `#t=`; the view itself is the client in WebAssembly.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

import init, { DisplayView } from './pkg/lispm_remote_client.js';

await init();

const token = location.hash.replace(/^#t=/, '');
const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
let socket = null;

const view = new DisplayView(document.getElementById('display'), (bytes) => {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(bytes);
  }
});

function connect() {
  socket = new WebSocket(`${scheme}://${location.host}/ws?t=${token}`);
  socket.binaryType = 'arraybuffer';
  socket.onopen = () => view.connected();
  socket.onmessage = (event) => view.receive(new Uint8Array(event.data));
  socket.onclose = () => {
    view.disconnected();
    setTimeout(connect, 2000);
  };
}

connect();
