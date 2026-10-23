// lispm-remote: the same, over the public network.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! LispM and the phone join Rings through a public seed, over the
//! internet, and pair and talk as in `remote`.  It needs the internet and
//! a seed that answers, so it runs only when asked:
//!
//! ```sh
//! cargo test --release -p lispm-remote-module --test rings -- --ignored
//! ```
//!
//! `LISPM_REMOTE_SEED` names another seed than `https://node.rings.rs`.

use crate::remote::Lispm;
use crate::remote::Phone;
use crate::remote::exchange;
use crate::remote::open_session;
use crate::remote::pair_over;
use crate::support::Peer;

#[test]
#[ignore = "needs the internet and a public seed"]
fn pairing_and_sessions_run_over_the_public_network() {
    let seed =
        std::env::var("LISPM_REMOTE_SEED").unwrap_or_else(|_| "https://node.rings.rs".to_owned());
    let mut a = Peer::start();
    let mut b = Peer::start();
    for peer in [&mut a, &mut b] {
        peer.ask("connectPeerViaHttp", serde_json::json!({ "url": seed }));
    }
    let mut lispm = Lispm::new(a, Vec::new());
    let mut phone = Phone::new(b, None);

    let (_, channel) = pair_over(&mut lispm, &mut phone);
    exchange(&mut lispm, &mut phone, channel, "agents?", "coder: idle");
    lispm.restart();
    phone.restart();
    let channel = open_session(&mut lispm, &mut phone);
    exchange(&mut lispm, &mut phone, channel, "approve 1", "approved");
}
