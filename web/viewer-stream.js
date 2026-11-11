/* Shared screenshot-stream viewer used by the iPhone and Android modules.
   Handles frames, scaling, gestures, fps stats and a self-healing connection. */

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
  let connectedOnce = false;
  let gesture = null;
  let pendingFrame = null;
  let drawing = false;
  let frames = 0;

  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socketUrl = `${protocol}://${location.host}${wsPath}`;

  let socket = null;
  let reconnectTimer = null;
  let attempts = 0;
  let heartbeatTimer = null;
  let watchdogTimer = null;
  let lastFrameAt = 0;
  let offline = !navigator.onLine;
  let wakeLock = null;

  const listeners = new AbortController();
  const { signal } = listeners;

  ui.setStatus("warn", "Connecting");
  ui.setStats("");

  const send = (message) => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
    }
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

  function keepAwake() {
    if (!window.isSecureContext || !navigator.wakeLock || wakeLock) return;
    navigator.wakeLock
      .request("screen")
      .then((lock) => {
        wakeLock = lock;
        lock.addEventListener("release", () => {
          wakeLock = null;
        });
      })
      .catch(() => {
        /* wake lock is best-effort */
      });
  }

  function handleMessage(message) {
    switch (message.type) {
      case "ready":
        ready = true;
        connectedOnce = true;
        attempts = 0;
        lastFrameAt = Date.now();
        scale = Number(message.scale) > 0 ? Number(message.scale) : 1;
        resizeCanvas(message.width, message.height);
        ui.hideOverlay();
        ui.setStatus("live", "Live");
        keepAwake();
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

  function stopHeartbeat() {
    window.clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }

  function startHeartbeat() {
    stopHeartbeat();
    heartbeatTimer = window.setInterval(() => {
      send({ type: "ping" });
    }, 15_000);
  }

  function scheduleReconnect(reason) {
    if (stopped) return;
    if (offline) {
      ui.setStatus("warn", "Waiting for network");
      ui.setStats("");
      ui.showOverlay("Waiting for network…", "Reconnecting automatically when the connection returns.", {
        spinner: true,
      });
      return;
    }

    attempts += 1;
    const delay = Math.min(15_000, 1_000 * 2 ** Math.min(attempts - 1, 4));
    window.clearTimeout(reconnectTimer);
    ui.setStatus("warn", "Reconnecting…");
    ui.setStats("");
    ui.showOverlay(
      connectedOnce ? "Reconnecting…" : "Connecting…",
      `${reason} Retrying automatically in ${Math.ceil(delay / 1000)}s.`,
      { spinner: true },
    );
    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delay);
  }

  function connect() {
    if (stopped || offline) return;
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
    stopHeartbeat();

    if (socket) {
      try {
        socket.close();
      } catch {
        /* already closed */
      }
    }

    ready = false;
    pendingFrame = null;
    const current = new WebSocket(socketUrl);
    current.binaryType = "blob";
    socket = current;

    current.addEventListener("open", () => {
      if (stopped || socket !== current) return;
      ui.setStatus("warn", "Waiting for frames");
      startHeartbeat();
    });

    current.addEventListener("message", (event) => {
      if (stopped || socket !== current) return;
      if (typeof event.data === "string") {
        try {
          handleMessage(JSON.parse(event.data));
        } catch {
          /* ignore malformed control frames */
        }
        return;
      }
      lastFrameAt = Date.now();
      pendingFrame = event.data;
      drainFrames();
    });

    current.addEventListener("error", () => {
      if (!stopped && ready) ui.toast("The device connection reported an error.", "error");
    });

    current.addEventListener("close", () => {
      if (stopped || socket !== current) return;
      stopHeartbeat();
      ready = false;
      scheduleReconnect(connectedOnce ? "The stream stopped." : deviceHint);
    });
  }

  watchdogTimer = window.setInterval(() => {
    if (stopped || !ready || offline) return;
    if (Date.now() - lastFrameAt > 8_000) {
      // A silently dead socket should be replaced instead of freezing the screen.
      ui.setStatus("warn", "Reconnecting…");
      ui.showOverlay("Reconnecting…", "The stream stalled. Reconnecting automatically.", {
        spinner: true,
      });
      connect();
    }
  }, 2_000);

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
    keepAwake();
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

  window.addEventListener("offline", () => {
    offline = true;
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
    ui.setStatus("warn", "Waiting for network");
    ui.setStats("");
    ui.showOverlay("Waiting for network…", "The phone lost its network connection. It will reconnect automatically.", {
      spinner: true,
    });
    try {
      socket?.close();
    } catch {
      /* already closed */
    }
  }, { signal });

  window.addEventListener("online", () => {
    offline = false;
    if (stopped) return;
    attempts = 0;
    connect();
  }, { signal });

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && !stopped && !ready && !offline) {
      attempts = 0;
      connect();
    }
    if (document.visibilityState === "visible") keepAwake();
  }, { signal });

  connect();

  return {
    actions,
    destroy() {
      stopped = true;
      listeners.abort();
      window.clearInterval(fpsTimer);
      window.clearInterval(watchdogTimer);
      window.clearInterval(heartbeatTimer);
      window.clearTimeout(reconnectTimer);
      wakeLock?.release().catch(() => {});
      wakeLock = null;
      if (socket) {
        try {
          socket.close();
        } catch {
          /* already closed */
        }
      }
      screen.replaceChildren();
    },
  };
}
