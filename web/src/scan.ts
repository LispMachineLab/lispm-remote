// lispm-remote: reading LispM's QR code with the camera.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * LispM's QR code is a URL, `https://lispm.ai/remote/#TOKEN`; the token is
 * what follows `#` (or `?token=`), which browsers do not send to the server
 * when carried in the hash.  Scanning the QR code with the device's camera
 * opens the page with the token in its URL; the page takes the token,
 * removes it from the address bar, and keeps it in `sessionStorage` until
 * pairing finishes so a reload does not lose it.
 *
 * When the page is opened on its own instead, the camera can also read the
 * code in the page through `getUserMedia` and jsQR (vendor/jsQR.js, loaded
 * as a classic script).
 */

import { expiry } from "./steps.js";

const PENDING_TOKEN = "lispm-remote.token";

interface Code {
  readonly data: string;
}

declare const jsQR: (
  data: Uint8ClampedArray,
  width: number,
  height: number,
  options?: { inversionAttempts?: "dontInvert" | "onlyInvert" | "attemptBoth" | "invertFirst" },
) => Code | null;

/** The widest image the decoder is given; larger frames are scaled down. */
const WIDTH = 720;

/** The offer token in `text`: what follows `#` or `?token=` of LispM's URL,
 * or the token alone.  Null when `text` holds none. */
export function tokenOf(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) {
    return null;
  }
  let token = trimmed;
  if (token.includes("#")) {
    token = token.slice(token.lastIndexOf("#") + 1);
    if (token.startsWith("token=")) {
      token = token.slice("token=".length);
    }
  } else if (token.includes("?") || token.startsWith("token=")) {
    const search = token.includes("?") ? token.slice(token.indexOf("?") + 1) : token;
    token = new URLSearchParams(search).get("token") ?? "";
  }
  return /^[A-Za-z0-9_-]{16,}$/.test(token) ? token : null;
}

/** Keeps `token` for this tab until pairing settles, so a reload while
 * joining does not drop the offer a camera scan opened the page with. */
export function keepToken(token: string): void {
  try {
    sessionStorage.setItem(PENDING_TOKEN, token);
  } catch {
    // Storage is a convenience across reloads.
  }
}

/** Drops the pending offer token kept for this tab. */
export function clearToken(): void {
  try {
    sessionStorage.removeItem(PENDING_TOKEN);
  } catch {
    // Ignore when sessionStorage is unavailable.
  }
}

function keptToken(now = Date.now()): string | null {
  try {
    const stored = sessionStorage.getItem(PENDING_TOKEN);
    if (!stored) {
      return null;
    }
    const token = tokenOf(stored);
    const expires = token ? expiry(token) : null;
    if (token && (expires === null || expires > now)) {
      return token;
    }
    sessionStorage.removeItem(PENDING_TOKEN);
    return null;
  } catch {
    return null;
  }
}

/** Takes the offer token from the page's own URL, the way a phone's camera
 * app opens the code, removes it from the URL and the history, and keeps it
 * in `sessionStorage` until pairing settles. */
export function takeToken(): string | null {
  const query = new URLSearchParams(location.search);
  const fromHash = location.hash ? tokenOf(location.hash) : null;
  const fromQuery = query.has("token") ? tokenOf(query.get("token") ?? "") : null;
  const token = fromHash ?? fromQuery;
  if (location.hash || query.has("token")) {
    query.delete("token");
    const search = query.toString();
    history.replaceState(null, "", `${location.pathname}${search ? `?${search}` : ""}`);
  }
  if (token) {
    keepToken(token);
    return token;
  }
  return keptToken();
}

/** Shows the camera in `video` until it sees a code with an offer, and
 * returns the offer's token.  Rejects when `signal` aborts or the camera
 * cannot be used. */
export async function scan(video: HTMLVideoElement, signal: AbortSignal): Promise<string> {
  const stream = await navigator.mediaDevices.getUserMedia({
    video: { facingMode: { ideal: "environment" } },
    audio: false,
  });
  const stop = () => {
    for (const track of stream.getTracks()) {
      track.stop();
    }
    video.srcObject = null;
  };
  try {
    video.srcObject = stream;
    await video.play();
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) {
      throw new Error("This browser cannot read the camera's pictures.");
    }
    return await new Promise<string>((resolve, reject) => {
      const abort = () => reject(new DOMException("The scan was cancelled.", "AbortError"));
      signal.addEventListener("abort", abort, { once: true });
      const look = () => {
        if (signal.aborted) {
          return;
        }
        if (video.readyState >= video.HAVE_CURRENT_DATA && video.videoWidth > 0) {
          const scale = Math.min(1, WIDTH / video.videoWidth);
          canvas.width = Math.round(video.videoWidth * scale);
          canvas.height = Math.round(video.videoHeight * scale);
          context.drawImage(video, 0, 0, canvas.width, canvas.height);
          const image = context.getImageData(0, 0, canvas.width, canvas.height);
          const code = jsQR(image.data, image.width, image.height, {
            inversionAttempts: "dontInvert",
          });
          const token = code && tokenOf(code.data);
          if (token) {
            signal.removeEventListener("abort", abort);
            resolve(token);
            return;
          }
        }
        requestAnimationFrame(look);
      };
      look();
    });
  } finally {
    stop();
  }
}
