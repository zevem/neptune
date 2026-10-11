// Neptune recorder: runs in a blank page the host owns. The host pushes page
// screenshots (JPEG) and pointer state; this composites them on a canvas and
// encodes the canvas with MediaRecorder. Chunks stay here until the host pulls
// them as base64 after stop().
(() => {
  const MIMES = ["video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"];
  const ARROW = new Path2D("M1 1v18l4-4 4 8 3-1.5-4-8H15Z");
  const RING_MS = 400;
  const MAX_WAITING = 3; // undecoded frames kept; older ones are dropped
  const TICK_MS = 33;
  const EMPTY = { mime: "", chunks: 0, bytes: 0, width: 0, height: 0, full: false };

  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d", { alpha: false });
  const chunks = [];
  const pointer = { x: -1, y: -1, down: false, visible: false };

  let waiting = []; // { b64, cssWidth } not yet decoded
  let pumping = null; // promise of the running decode loop
  let bitmap = null; // last page frame, kept for cursor-only redraws
  let place = { x: 0, y: 0, w: 0, h: 0 }; // where the bitmap sits on the canvas
  let scale = 1; // canvas px per CSS px of the recorded page
  let rings = []; // { x, y, t } press points still animating
  let recorder = null;
  let track = null;
  let mime = "";
  let bytes = 0;
  let frames = 0;
  let dirty = false; // the pointer changed since the last composite
  let timer = 0;
  let stopping = null;
  let stopped = false;

  const compose = () => {
    if (!bitmap) return;
    const now = performance.now();
    const covers = place.x === 0 && place.y === 0 && place.w === canvas.width && place.h === canvas.height;
    if (!covers) {
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    }
    ctx.drawImage(bitmap, place.x, place.y, place.w, place.h);
    if (pointer.visible) {
      ctx.save();
      ctx.translate(place.x + pointer.x * scale, place.y + pointer.y * scale);
      ctx.scale(scale, scale);
      ctx.translate(-1, -1); // the path's tip is at (1, 1)
      ctx.fillStyle = "#000";
      ctx.fill(ARROW);
      ctx.strokeStyle = "#fff";
      ctx.lineWidth = 1.5;
      ctx.lineJoin = "round";
      ctx.stroke(ARROW);
      ctx.restore();
    }
    rings = rings.filter((ring) => now - ring.t < RING_MS);
    for (const ring of rings) {
      const age = (now - ring.t) / RING_MS;
      ctx.beginPath();
      ctx.arc(place.x + ring.x * scale, place.y + ring.y * scale, (8 + 18 * age) * scale, 0, Math.PI * 2);
      ctx.strokeStyle = `rgba(76,141,255,${(0.9 * (1 - age)).toFixed(3)})`;
      ctx.lineWidth = 3 * scale;
      ctx.stroke();
    }
    dirty = false;
  };

  // The pointer and press rings must keep moving while the page is not
  // repainting, so redraw from the last bitmap, but only while something changes.
  const tick = () => {
    if (stopped || api.full || (!dirty && rings.length === 0)) {
      clearInterval(timer);
      timer = 0;
      return;
    }
    compose();
  };
  const schedule = () => {
    if (!timer && bitmap && !stopped && !api.full) timer = setInterval(tick, TICK_MS);
  };

  const startRecorder = () => {
    const stream = canvas.captureStream(30);
    track = stream.getVideoTracks()[0];
    mime = MIMES.find((type) => MediaRecorder.isTypeSupported(type)) ?? "";
    const videoBitsPerSecond = Math.round(
      Math.min(20e6, Math.max(2e6, canvas.width * canvas.height * 30 * 0.05)),
    );
    recorder = new MediaRecorder(stream, mime ? { mimeType: mime, videoBitsPerSecond } : { videoBitsPerSecond });
    recorder.ondataavailable = (event) => {
      if (!event.data || event.data.size === 0) return;
      chunks.push(event.data);
      bytes += event.data.size;
      if (bytes > api.maxBytes && !api.full) {
        // Bound memory: take no more frames and stop the encoder producing data.
        api.full = true;
        waiting = [];
        if (recorder.state === "recording") recorder.pause();
      }
    };
    recorder.start(1000);
  };

  const draw = (next, cssWidth) => {
    if (!recorder) {
      // The first frame fixes the video size; encoders want even dimensions.
      canvas.width = next.width + (next.width & 1);
      canvas.height = next.height + (next.height & 1);
      startRecorder();
    }
    bitmap?.close();
    bitmap = next;
    const fits = next.width <= canvas.width && next.height <= canvas.height &&
      canvas.width - next.width <= 1 && canvas.height - next.height <= 1;
    // Same size (give or take the evening pixel) is drawn 1:1; anything else
    // is fitted inside the canvas, centred, on black.
    const fit = fits ? 1 : Math.min(canvas.width / next.width, canvas.height / next.height);
    const w = fits ? next.width : Math.round(next.width * fit);
    const h = fits ? next.height : Math.round(next.height * fit);
    place = fits
      ? { x: 0, y: 0, w, h }
      : { x: Math.round((canvas.width - w) / 2), y: Math.round((canvas.height - h) / 2), w, h };
    scale = cssWidth > 0 ? w / cssWidth : 1;
    compose();
    frames += 1;
    if (rings.length) schedule();
  };

  const pump = async () => {
    while (waiting.length && !api.full && !stopped) {
      const { b64, cssWidth } = waiting.shift();
      let next;
      try {
        next = await createImageBitmap(await (await fetch("data:image/jpeg;base64," + b64)).blob());
      } catch {
        continue; // an undecodable frame is skipped, not fatal
      }
      if (stopped || api.full) {
        next.close();
        break;
      }
      draw(next, cssWidth);
    }
  };

  const api = {
    maxBytes: 256 * 1024 * 1024,
    full: false,

    frame(b64, cssWidth) {
      if (api.full || stopping) return;
      waiting.push({ b64, cssWidth });
      if (waiting.length > MAX_WAITING) waiting.shift();
      pumping ??= pump().finally(() => {
        pumping = null;
      });
    },

    cursor(x, y, down) {
      const visible = x >= 0;
      if (visible && down && !pointer.down) rings.push({ x, y, t: performance.now() });
      pointer.x = x;
      pointer.y = y;
      pointer.down = Boolean(down);
      pointer.visible = visible;
      dirty = true;
      schedule();
    },

    stop() {
      stopping ??= (async () => {
        if (pumping) await pumping; // frames already accepted still get drawn
        stopped = true;
        clearInterval(timer);
        timer = 0;
        waiting = [];
        if (!recorder) return { ...EMPTY };
        if (recorder.state !== "inactive") {
          compose();
          track.requestFrame?.();
          // The forced frame reaches the encoder asynchronously.
          await new Promise((done) => setTimeout(done, 60));
          const ended = new Promise((done) => recorder.addEventListener("stop", done, { once: true }));
          recorder.stop();
          await ended;
        }
        track.stop();
        bitmap?.close();
        bitmap = null;
        return {
          mime: recorder.mimeType || mime,
          chunks: chunks.length,
          bytes,
          width: canvas.width,
          height: canvas.height,
          full: api.full,
        };
      })();
      return stopping;
    },

    async chunk(i) {
      const blob = chunks[i];
      if (!blob) return "";
      const data = new Uint8Array(await blob.arrayBuffer());
      let binary = "";
      for (let at = 0; at < data.length; at += 0x8000) {
        binary += String.fromCharCode.apply(null, data.subarray(at, at + 0x8000));
      }
      return btoa(binary);
    },

    state() {
      return { frames, bytes, full: api.full };
    },
  };

  globalThis.__neptuneRecorder = api;
})();
