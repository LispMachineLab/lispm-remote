// lispm-remote: a handshake of the page's node that keeps it off Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * While the page's node holds a handshake with the seed that is not done,
 * Rings refuses to start another: a join fails with "a handshake with
 * 0x… is already in flight".  Most such handshakes finish in a second or
 * two.  One whose connection finds no path may never finish, and Rings
 * keeps it for three minutes (`PENDING_CONNECTION_TIMEOUT_MS`), all that
 * time refusing every join.  So the page drops a handshake that has
 * refused its joins for `STUCK_MS`, and the next join starts a new one.
 *
 * In WebKit (Safari and every browser on iOS), `RTCPeerConnection` gathers
 * its STUN `srflx` candidate in a few hundred milliseconds, yet its
 * `iceGatheringState` can stay `"gathering"` past Rings's 60 s gather
 * timeout while waiting on mDNS host-candidate registration or secondary
 * interfaces that never finish.  `settleIceGathering` marks gathering
 * `"complete"` once gathered candidates have settled.
 */

/** How long a handshake may refuse joins before the page drops it. */
export const STUCK_MS = 10_000;

/** How long after a reflexive or relay candidate ICE gathering waits for more. */
export const REFLEXIVE_SETTLE_MS = 300;

/** How long after a host-only candidate ICE gathering waits for STUN or TURN. */
export const HOST_SETTLE_MS = 3_000;

/** The settle wait, in milliseconds, for a candidate line or SDP `text`, or
 * null when `text` carries no candidate. */
export function gatherSettleDelay(text: string): number | null {
  if (/\btyp (?:srflx|relay)\b/.test(text)) {
    return REFLEXIVE_SETTLE_MS;
  }
  if (/\btyp host\b|^a=candidate:/m.test(text)) {
    return HOST_SETTLE_MS;
  }
  return null;
}

const SETTLED = Symbol.for("lispm-remote.settleIceGathering");

/** Ensures `scope.RTCPeerConnection` transitions `iceGatheringState` to
 * `"complete"` once gathered candidates have settled, even when WebKit's
 * mDNS or secondary-interface gatherer never finishes. */
export function settleIceGathering(
  scope: { RTCPeerConnection?: typeof RTCPeerConnection } = globalThis,
): void {
  const Native = scope.RTCPeerConnection as
    | (typeof RTCPeerConnection & { [SETTLED]?: boolean })
    | undefined;
  if (!Native || Native[SETTLED]) {
    return;
  }
  class SettlingPeerConnection extends Native {
    static [SETTLED] = true;
    private settled = false;
    private reflexive = false;
    private timer: ReturnType<typeof setTimeout> | null = null;

    constructor(...args: ConstructorParameters<typeof RTCPeerConnection>) {
      super(...args);
      this.addEventListener("icecandidate", (event: RTCPeerConnectionIceEvent) => {
        if (!event.candidate) {
          this.clearTimer();
          return;
        }
        if (event.candidate.candidate) {
          this.armTimer(event.candidate.candidate);
        }
      });
      this.addEventListener("icegatheringstatechange", () => {
        if (super.iceGatheringState === "complete") {
          this.clearTimer();
        }
      });
    }

    private clearTimer(): void {
      if (this.timer !== null) {
        clearTimeout(this.timer);
        this.timer = null;
      }
    }

    private armTimer(text: string): void {
      if (this.settled || super.iceGatheringState === "complete") {
        return;
      }
      if (/\btyp (?:srflx|relay)\b/.test(text)) {
        this.reflexive = true;
      }
      const delay = this.reflexive ? REFLEXIVE_SETTLE_MS : gatherSettleDelay(text);
      if (delay === null) {
        return;
      }
      this.clearTimer();
      this.timer = setTimeout(() => {
        this.timer = null;
        if (!this.settled && super.iceGatheringState !== "complete") {
          this.settled = true;
          this.dispatchEvent(new Event("icegatheringstatechange"));
        }
      }, delay);
    }

    override get iceGatheringState(): RTCIceGatheringState {
      return this.settled ? "complete" : super.iceGatheringState;
    }

    override async setLocalDescription(description?: RTCLocalSessionDescriptionInit): Promise<void> {
      this.settled = false;
      this.reflexive = false;
      this.clearTimer();
      await super.setLocalDescription(description);
      const sdp = this.localDescription?.sdp ?? "";
      if (this.timer === null && sdp) {
        this.armTimer(sdp);
      }
    }

    override close(): void {
      this.clearTimer();
      super.close();
    }
  }
  scope.RTCPeerConnection = SettlingPeerConnection;
}

/** A handshake refusing the page's joins: with which peer, and since when,
 * in milliseconds since the epoch. */
export interface Held {
  readonly peer: string;
  readonly since: number;
}

/** The peer of the handshake that the join failure `message` says is in
 * flight, or null when it says something else. */
export function inFlight(message: string): string | null {
  return /handshake with (0x[0-9a-f]+) is already in flight/i.exec(message)?.[1] ?? null;
}

/** The handshake refusing joins after one failed with `message` at `now`,
 * given `held`, the one that refused before: a refusal by the same peer
 * keeps the first one's time. */
export function holding(held: Held | null, message: string, now: number): Held | null {
  const peer = inFlight(message);
  if (peer === null) {
    return null;
  }
  return held?.peer === peer ? held : { peer, since: now };
}

/** Whether `held` has refused joins for long enough at `now` to be dropped. */
export function stuck(held: Held | null, now: number): held is Held {
  return held !== null && now - held.since >= STUCK_MS;
}
