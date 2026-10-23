// lispm-remote: reading LispM's QR code with the camera.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The camera, through `getUserMedia`, and jsQR (vendor/jsQR.js, loaded as
 * a classic script) to read the code: Chrome on Linux has no
 * BarcodeDetector, and one decoder everywhere behaves the same everywhere.
 *
 * LispM's code is a URL, `https://lispm.ai/remote/#TOKEN`; the token is
 * what follows the `#`.  Browsers do not send that part to the server.
 */

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

/** The offer token in `text`: what follows the `#` of LispM's URL, or the
 * token alone.  Null when `text` holds none. */
export function tokenOf(text: string): string | null {
  const token = text.includes("#") ? text.slice(text.lastIndexOf("#") + 1) : text;
  return /^[A-Za-z0-9_-]{16,}$/.test(token) ? token : null;
}

/** Takes the offer token from the page's own URL, the way a phone's camera
 * app opens the code, and removes it from the URL and the history. */
export function takeToken(): string | null {
  if (!location.hash) {
    return null;
  }
  const token = tokenOf(location.hash);
  history.replaceState(null, "", `${location.pathname}${location.search}`);
  return token;
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
