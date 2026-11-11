/* macOS/Windows viewer: noVNC over the server's WebSocket-to-TCP bridge.
   Reconnects automatically and keeps credentials in memory for the session. */

const NOVNC_VERSION = "1.7.0";
const NOVNC_CDN = `https://cdn.jsdelivr.net/npm/@novnc/novnc@${NOVNC_VERSION}/core/rfb.js`;

async function loadRfb(vendorAvailable) {
  // Prefer a vendored copy (offline/air-gapped installs), fall back to the CDN.
  if (vendorAvailable) {
    try {
      const local = await import("/vendor/novnc/core/rfb.js");
      return local.default;
    } catch {
      /* fall through to the CDN */
    }
  }
  try {
    const remote = await import(/* @vite-ignore */ NOVNC_CDN);
    return remote.default;
  } catch {
    throw new Error(
      "Could not load the noVNC client. Run scripts/vendor-novnc.sh to host it locally, or allow cdn.jsdelivr.net.",
    );
  }
}

/**
 * Ask for either a VNC password or macOS account credentials. noVNC reports
 * which fields the Mac expects (`password`, or `username` + `password` for
 * Apple Remote Desktop authentication).
 */
function askCredentials(types, hostName) {
  return new Promise((resolve) => {
    document.querySelector("dialog.cred-dialog")?.remove();
    const wantsUsername = types.includes("username");
    const dialog = document.createElement("dialog");
    dialog.className = "dialog cred-dialog";
    dialog.innerHTML = `
      <form>
        <div class="dialog-head">
          <div>
            <h2>Sign in to the computer</h2>
            <p class="section-note" data-role="note"></p>
          </div>
          <button class="icon-btn" type="button" data-role="close" aria-label="Cancel">
            <svg class="icon"><use href="#i-x"></use></svg>
          </button>
        </div>
        <div class="dialog-body">
          <label class="field cred-username">
            <span>Username</span>
            <input name="username" autocomplete="username" autocapitalize="off" spellcheck="false" />
          </label>
          <label class="field">
            <span>Password</span>
            <input name="password" type="password" autocomplete="current-password" required />
          </label>
          <p class="hint">
            Use the VNC password configured on the remote computer, or the
            account credentials if it asks for a username. Credentials go
            straight to the device and are never stored by Sethu.
          </p>
        </div>
        <div class="dialog-foot">
          <button class="btn btn-ghost" type="button" data-role="close">Cancel</button>
          <button class="btn btn-primary" type="submit">Sign in</button>
        </div>
      </form>`;

    dialog.querySelector('[data-role="note"]').textContent =
      `${hostName} is asking for credentials.`;
    const usernameLabel = dialog.querySelector(".cred-username");
    const usernameInput = dialog.querySelector('input[name="username"]');
    if (!wantsUsername) {
      usernameLabel.classList.add("hidden");
    } else {
      usernameInput.required = true;
    }

    const form = dialog.querySelector("form");
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      resolve(value);
      dialog.close();
      dialog.remove();
    };

    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const data = new FormData(form);
      finish({
        password: String(data.get("password") || ""),
        username: wantsUsername ? String(data.get("username") || "") : undefined,
      });
    });
    dialog.querySelectorAll('[data-role="close"]').forEach((button) => {
      button.addEventListener("click", () => finish(null));
    });
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      finish(null);
    });
    dialog.addEventListener("close", () => finish(null));

    document.body.append(dialog);
    dialog.showModal();
    window.setTimeout(() => {
      (wantsUsername ? usernameInput : dialog.querySelector('input[name="password"]')).focus();
    }, 50);
  });
}

export async function mountVnc({ host, screen, ui, vendorNovnc }) {
  ui.showOverlay("Connecting…", "Negotiating screen sharing with the computer.", { spinner: true });

  const RFB = await loadRfb(Boolean(vendorNovnc));
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const url = `${protocol}://${location.host}/ws/vnc/${host.id}`;
  const options = {
    shared: true,
    wsProtocols: ["binary"],
    scaleViewport: true,
    resizeSession: false,
    clipViewport: false,
    showDotCursor: true,
    focusOnClick: true,
    background: "#02040a",
  };

  let rfb = null;
  let stopped = false;
  let cancelled = false;
  let connected = false;
  let connectedOnce = false;
  let credentials = null;
  let credentialsPending = false;
  let reconnectTimer = null;
  let attempts = 0;
  let fit = true;
  let viewOnly = false;
  let qualityLevel = 6;
  let compressionLevel = 4;
  let wakeLock = null;

  const listeners = new AbortController();
  const { signal } = listeners;

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
        /* best-effort */
      });
  }

  function stopSocket(socket) {
    if (!socket) return;
    try {
      socket.disconnect();
    } catch {
      /* already closed */
    }
  }

  function scheduleReconnect(reason) {
    if (stopped || cancelled) return;
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
    if (stopped) return;
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;

    if (rfb) {
      const old = rfb;
      rfb = null;
      stopSocket(old);
    }
    screen.replaceChildren();
    connected = false;
    cancelled = false;

    const socket = new RFB(screen, url, options);
    socket.scaleViewport = fit;
    socket.viewOnly = viewOnly;
    socket.qualityLevel = qualityLevel;
    socket.compressionLevel = compressionLevel;
    rfb = socket;

    socket.addEventListener("connect", () => {
      if (rfb !== socket) return;
      connected = true;
      connectedOnce = true;
      attempts = 0;
      ui.hideOverlay();
      ui.setStatus("live", "Connected");
      ui.setStats("");
      keepAwake();
    });

    socket.addEventListener("disconnect", (event) => {
      if (stopped || rfb !== socket) return;
      connected = false;
      if (cancelled) {
        ui.setStatus("off", "Sign-in cancelled");
        ui.setStats("");
        ui.showOverlay(
          "Sign-in cancelled",
          "Reconnect and enter your account or VNC password to continue.",
          { retry: true },
        );
        return;
      }
      const clean = Boolean(event.detail && event.detail.clean);
      scheduleReconnect(clean ? "The screen-sharing session ended." : "The connection dropped.");
    });

    socket.addEventListener("credentialsrequired", async (event) => {
      if (rfb !== socket || credentialsPending) return;
      credentialsPending = true;
      const types = (event.detail && event.detail.types) || ["password"];
      try {
        if (credentials && (!types.includes("username") || credentials.username)) {
          // Same device, same session: reuse the in-memory credentials so a
          // reconnect is seamless. They are never written to storage.
          socket.sendCredentials(credentials);
          return;
        }
        const entered = await askCredentials(types, host.name);
        if (!entered) {
          cancelled = true;
          stopSocket(socket);
          return;
        }
        credentials = entered;
        socket.sendCredentials(entered);
      } catch {
        ui.toast("Could not send the credentials to the computer.", "error");
      } finally {
        credentialsPending = false;
      }
    });

    socket.addEventListener("securityfailure", (event) => {
      credentials = null;
      const reason = event.detail && event.detail.reason;
      ui.toast(
        `Authentication failed${reason ? `: ${reason}` : ""}. Check the VNC password or account credentials.`,
        "error",
      );
    });

    socket.addEventListener("desktopname", (event) => {
      const name = event.detail && event.detail.name;
      if (name) ui.setStats(name);
    });
  }

  const actions = [
    {
      id: "fit",
      label: "Fit",
      icon: "i-expand",
      active: true,
      run: () => {
        fit = !fit;
        if (rfb) rfb.scaleViewport = fit;
        return fit;
      },
    },
    {
      id: "viewonly",
      label: "View only",
      icon: "i-eye",
      active: false,
      run: () => {
        viewOnly = !viewOnly;
        if (rfb) rfb.viewOnly = viewOnly;
        return viewOnly;
      },
    },
    {
      type: "select",
      label: "Quality",
      value: "6",
      options: [
        { value: "9", label: "Best" },
        { value: "6", label: "Balanced" },
        { value: "3", label: "Fast" },
        { value: "1", label: "Lowest" },
      ],
      onChange: (value) => {
        qualityLevel = Number(value);
        compressionLevel = qualityLevel >= 6 ? 2 : qualityLevel >= 3 ? 4 : 7;
        if (rfb) {
          rfb.qualityLevel = qualityLevel;
          rfb.compressionLevel = compressionLevel;
        }
      },
    },
  ];

  window.addEventListener("online", () => {
    if (stopped || cancelled) return;
    if (!connected && !credentialsPending && !document.querySelector("dialog.cred-dialog")) {
      attempts = 0;
      connect();
    }
  }, { signal });

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      keepAwake();
      if (
        !cancelled &&
        !connected &&
        !credentialsPending &&
        !document.querySelector("dialog.cred-dialog")
      ) {
        attempts = 0;
        connect();
      }
    }
  }, { signal });

  screen.addEventListener("pointerdown", keepAwake, { signal });

  connect();

  return {
    actions,
    destroy() {
      stopped = true;
      listeners.abort();
      window.clearTimeout(reconnectTimer);
      wakeLock?.release().catch(() => {});
      wakeLock = null;
      credentials = null;
      const old = rfb;
      rfb = null;
      stopSocket(old);
      screen.replaceChildren();
    },
  };
}
