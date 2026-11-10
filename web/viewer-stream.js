/* Shared screenshot-stream viewer used by the iPhone and Android modules.
   Handles frames, scaling, gestures, fps stats and connection lifecycle. */

export async function mountStream({
  host,
  wsPath,
  screen,
  ui,
  deviceHint,
  buildActions,
}) {
  const canvas = document.createElement("canvas");
  canvas.className = "stream-canvas";
  canvas.setAttribute("aria-label", `${host.name} screen`);
  screen.replaceChildren(canvas);

  const context = canvas.getContext("2d", { alpha: false });
  let dims = { w: 0, h: 0 };
  let scale = 1;
  let stopped = false;
  let ready = false;
  let gesture = null;
  let pendingFrame = null;
  let drawing = false;
  let frames = 0;

  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocol}://${location.host}${wsPath}`);
  socket.binaryType = "blob";

  ui.setStatus("warn", "Connecting");
  ui.setStats("");

  const send = (message) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };

  function resizeCanvas(width, height) {
    if (!width || !height) return;
    dims = { w: width, h: height };
    canvas.width = width;
    canvas.height = height;
    canvas.style.aspectRatio = `${width} / ${height}`;
  }

  async function drainFrames() {
    if (drawing) return;
    drawing = true;
    while (pendingFrame && !stopped) {
      const blob = pendingFrame;
      pendingFrame = null;
      const bitmap = await createImageBitmap(blob).catch(() => null);
      if (!bitmap) continue;
      if (bitmap.width !== dims.w || bitmap.height !== dims.h) {
        resizeCanvas(bitmap.width, bitmap.height);
      }
      context.drawImage(bitmap, 0, 0, bitmap.width, bitmap.height);
      bitmap.close();
      frames += 1;
    }
    drawing = false;
  }

  function handleMessage(message) {
    switch (message.type) {
      case "ready":
        scale = Number(message.scale) > 0 ? Number(message.scale) : 1;
        ready = true;
        resizeCanvas(message.width, message.height);
        ui.hideOverlay();
        ui.setStatus("live", "Live");
        break;
      case "size":
        resizeCanvas(message.width, message.height);
        break;
      case "error":
        ui.toast(message.message, "error");
        break;
      case "fps":
        break;
      default:
        break;
    }
  }

  socket.addEventListener("message", (event) => {
    if (typeof event.data === "string") {
      try {
        handleMessage(JSON.parse(event.data));
      } catch {
        /* ignore malformed control frames */
      }
      return;
    }
    pendingFrame = event.data;
    drainFrames();
  });

  socket.addEventListener("open", () => {
    ui.setStatus("warn", "Waiting for frames");
  });

  socket.addEventListener("error", () => {
    if (!stopped && ready) ui.toast("The device connection reported an error.", "error");
  });

  socket.addEventListener("close", () => {
    if (stopped) return;
    if (!ready) {
      ui.setStatus("off", "Unavailable");
      ui.setStats("");
      ui.showOverlay("Could not connect", deviceHint, { retry: true });
      return;
    }
    ui.setStatus("off", "Connection lost");
    ui.setStats("");
    ui.showOverlay(
      "Connection lost",
      `${host.name} stopped sending frames. Check the device and reconnect.`,
      { retry: true },
    );
  });

  const fpsTimer = window.setInterval(() => {
    ui.setStats(frames > 0 ? `${frames} fps` : "");
    frames = 0;
  }, 1000);

  function toDevice(clientX, clientY) {
    const rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height || !dims.w) return { x: 0, y: 0 };
    return {
      x: (((clientX - rect.left) / rect.width) * dims.w) / scale,
      y: (((clientY - rect.top) / rect.height) * dims.h) / scale,
    };
  }

  canvas.addEventListener("pointerdown", (event) => {
    if (!dims.w) return;
    event.preventDefault();
    canvas.setPointerCapture(event.pointerId);
    gesture = {
      startX: event.clientX,
      startY: event.clientY,
      at: performance.now(),
    };
  });

  canvas.addEventListener("pointerup", (event) => {
    if (!gesture) return;
    const started = gesture;
    gesture = null;
    const distance = Math.hypot(event.clientX - started.startX, event.clientY - started.startY);
    const from = toDevice(started.startX, started.startY);
    const to = toDevice(event.clientX, event.clientY);
    const duration = Math.max(0.08, (performance.now() - started.at) / 1000);

    if (distance < 12) {
      send({ type: "tap", x: from.x, y: from.y });
    } else {
      send({
        type: "swipe",
        x1: from.x,
        y1: from.y,
        x2: to.x,
        y2: to.y,
        duration: Math.min(duration, 2),
      });
    }
  });

  canvas.addEventListener("pointercancel", () => {
    gesture = null;
  });

  canvas.addEventListener("contextmenu", (event) => event.preventDefault());

  function saveScreenshot() {
    if (!dims.w) {
      ui.toast("No frame has arrived yet.", "error");
      return;
    }
    canvas.toBlob((blob) => {
      if (!blob) return;
      const link = document.createElement("a");
      const slug =
        host.name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "device";
      link.href = URL.createObjectURL(blob);
      link.download = `${slug}-${new Date().toISOString().replace(/[:.]/g, "-")}.png`;
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(link.href), 5000);
      ui.toast("Screenshot saved.", "success");
    }, "image/png");
  }

  const actions = buildActions({ send, saveScreenshot, host, ui });

  return {
    actions,
    destroy() {
      stopped = true;
      window.clearInterval(fpsTimer);
      try {
        socket.close();
      } catch {
        /* already closed */
      }
      screen.replaceChildren();
    },
  };
}
