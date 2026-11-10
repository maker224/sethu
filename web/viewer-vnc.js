/* macOS viewer: noVNC over the server's WebSocket-to-TCP bridge. */

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
            <h2>Sign in to the Mac</h2>
            <p class="section-note" data-role="note"></p>
          </div>
          <button class="icon-btn" type="button" data-role="close" aria-label="Cancel">
            <svg class="icon"><use href="#i-x"></use></svg>
          </button>
        </div>
        <div class="dialog-body">
          <label class="field cred-username">
            <span>macOS username</span>
            <input name="username" autocomplete="username" autocapitalize="off" spellcheck="false" />
          </label>
          <label class="field">
            <span>Password</span>
            <input name="password" type="password" autocomplete="current-password" required />
          </label>
          <p class="hint">
            Use your macOS account, or the VNC password if you enabled it in
            Screen Sharing → Computer Settings. Credentials go straight to the
            Mac and are never stored by Sethu.
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
  ui.showOverlay("Connecting…", "Negotiating screen sharing with the Mac.", { spinner: true });

  const RFB = await loadRfb(Boolean(vendorNovnc));
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const url = `${protocol}://${location.host}/ws/vnc/${host.id}`;

  const rfb = new RFB(screen, url, {
    shared: true,
    wsProtocols: ["binary"],
    scaleViewport: true,
    resizeSession: false,
    clipViewport: false,
    showDotCursor: true,
    focusOnClick: true,
    background: "#02040a",
  });

  let stopped = false;
  let connected = false;
  let cancelled = false;
  let ended = false;
  let credentialsPending = false;

  rfb.addEventListener("connect", () => {
    connected = true;
    ui.hideOverlay();
    ui.setStatus("live", "Connected");
    ui.setStats("");
  });

  rfb.addEventListener("disconnect", (event) => {
    if (stopped) return;
    ended = true;

    if (cancelled) {
      ui.setStatus("off", "Sign-in cancelled");
      ui.setStats("");
      ui.showOverlay(
        "Sign-in cancelled",
        "Reconnect and enter your macOS account or the VNC password to continue.",
        { retry: true },
      );
      return;
    }

    const clean = Boolean(event.detail && event.detail.clean);
    if (!connected) {
      ui.setStatus("off", "Unavailable");
      ui.setStats("");
      ui.showOverlay(
        "Could not connect",
        `The server could not open a Screen Sharing session with ${host.target}. Check that Screen Sharing is on, the address and port are right, and the Mac is reachable from this server.`,
        { retry: true },
      );
      return;
    }

    ui.setStatus("off", clean ? "Disconnected" : "Connection lost");
    ui.setStats("");
    ui.showOverlay(
      clean ? "Session ended" : "Connection lost",
      clean
        ? "The Mac closed the screen sharing session."
        : "The Mac or the network dropped the session. Reconnect to continue.",
      { retry: true },
    );
  });

  rfb.addEventListener("credentialsrequired", async (event) => {
    if (credentialsPending) return;
    credentialsPending = true;
    const types = (event.detail && event.detail.types) || ["password"];
    try {
      const credentials = await askCredentials(types, host.name);
      if (!credentials) {
        cancelled = true;
        try {
          rfb.disconnect();
        } catch {
          /* already closed */
        }
        return;
      }
      try {
        rfb.sendCredentials(credentials);
      } catch {
        ui.toast("Could not send the credentials to the Mac.", "error");
      }
    } finally {
      credentialsPending = false;
    }
  });

  rfb.addEventListener("securityfailure", (event) => {
    const reason = event.detail && event.detail.reason;
    ui.toast(
      `Authentication failed${reason ? `: ${reason}` : ""}. Check your macOS username/password or the VNC password.`,
      "error",
    );
  });

  rfb.addEventListener("desktopname", (event) => {
    const name = event.detail && event.detail.name;
    if (name) ui.setStats(name);
  });

  const actions = [
    {
      id: "fit",
      label: "Fit",
      icon: "i-expand",
      active: true,
      run: () => {
        rfb.scaleViewport = !rfb.scaleViewport;
        return rfb.scaleViewport;
      },
    },
    {
      id: "viewonly",
      label: "View only",
      icon: "i-eye",
      active: false,
      run: () => {
        rfb.viewOnly = !rfb.viewOnly;
        return rfb.viewOnly;
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
        const level = Number(value);
        rfb.qualityLevel = level;
        rfb.compressionLevel = level >= 6 ? 2 : level >= 3 ? 4 : 7;
      },
    },
  ];

  return {
    actions,
    destroy() {
      stopped = true;
      if (ended) return;
      try {
        rfb.disconnect();
      } catch {
        /* already closed */
      }
    },
  };
}
