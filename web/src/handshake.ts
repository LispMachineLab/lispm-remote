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

/** How long after an IPv4 reflexive or relay candidate ICE gathering waits for more. */
export const REFLEXIVE_SETTLE_MS = 500;

/** How long after a host or IPv6-only candidate ICE gathering waits for IPv4 STUN or TURN. */
export const HOST_SETTLE_MS = 3_000;

const IPV4_REFLEXIVE = /\b(?:\d{1,3}\.){3}\d{1,3}\s+\d+\s+typ (?:srflx|relay)\b/;

/** The settle wait, in milliseconds, for a candidate line or SDP `text`, or
 * null when `text` carries no candidate.  Only an IPv4 `srflx` or `relay`
 * candidate takes the short settle window, so an IPv6 cellular candidate on
 * a dual-stack phone does not cut off gathering before the IPv4 STUN
 * candidate needed for an IPv4 seed arrives. */
export function gatherSettleDelay(text: string): number | null {
  if (IPV4_REFLEXIVE.test(text)) {
    return REFLEXIVE_SETTLE_MS;
  }
  if (/\btyp (?:host|srflx|relay|prflx)\b|^a=candidate:/m.test(text)) {
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
    private notified = false;
    private gatheringHandler: ((this: RTCPeerConnection, ev: Event) => unknown) | null = null;
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

    private finishGathering(): void {
      this.timer = null;
      if (this.settled || super.iceGatheringState === "complete") {
        return;
      }
      this.settled = true;
      this.notified = false;
      const event = new Event("icegatheringstatechange");
      this.dispatchEvent(event);
      if (!this.notified && typeof this.gatheringHandler === "function") {
        this.gatheringHandler.call(this, event);
      }
    }

    private armTimer(text: string, fallbackMs: number | null = null): void {
      if (this.settled || super.iceGatheringState === "complete") {
        return;
      }
      if (IPV4_REFLEXIVE.test(text)) {
        this.reflexive = true;
      }
      const delay = this.reflexive
        ? REFLEXIVE_SETTLE_MS
        : (gatherSettleDelay(text) ?? fallbackMs);
      if (delay === null) {
        return;
      }
      this.clearTimer();
      this.timer = setTimeout(() => this.finishGathering(), delay);
    }

    override get onicegatheringstatechange(): ((this: RTCPeerConnection, ev: Event) => unknown) | null {
      return this.gatheringHandler;
    }

    override set onicegatheringstatechange(
      handler: ((this: RTCPeerConnection, ev: Event) => unknown) | null,
    ) {
      this.gatheringHandler = handler;
      super.onicegatheringstatechange = handler
        ? (event: Event) => {
            this.notified = true;
            return handler.call(this, event);
          }
        : null;
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
      if (this.timer === null) {
        this.armTimer(sdp, HOST_SETTLE_MS);
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
