/* Session shell: loads the device, mounts the right viewer and wires the
   shared toolbar. Viewer modules own the transport and return actions. */

import { mountVnc } from "./viewer-vnc.js?v=0.7.1";
import { mountIos } from "./viewer-ios.js?v=0.7.1";
import { mountAndroid } from "./viewer-android.js?v=0.7.1";

const $ = (selector) => document.querySelector(selector);

const els = {
  name: $("#host-name"),
  target: $("#host-target"),
  stats: $("#stats"),
  dot: $("#status-dot"),
  screen: $("#screen"),
  overlay: $("#overlay"),
  overlayTitle: $("#overlay-title"),
  overlayDetail: $("#overlay-detail"),
  overlaySpinner: $("#overlay-spinner"),
  overlayRetry: $("#overlay-retry"),
  actions: $("#viewer-actions"),
  fullscreen: $("#fullscreen"),
  disconnect: $("#disconnect"),
  toasts: $("#toasts"),
  stage: $("#stage"),
};

let activeViewer = null;

function toast(message, kind = "info") {
  const node = document.createElement("div");
  node.className = `toast${kind === "error" ? " is-error" : kind === "success" ? " is-success" : ""}`;
  node.textContent = message;
  els.toasts.append(node);
  while (els.toasts.children.length > 3) els.toasts.firstElementChild.remove();
  window.setTimeout(() => {
    node.classList.add("is-leaving");
    window.setTimeout(() => node.remove(), 200);
  }, kind === "error" ? 6000 : 3600);
}

function setStatus(kind, label) {
  const classes = ["status-dot"];
  if (kind === "live") classes.push("is-live");
  else if (kind === "warn") classes.push("is-warn");
  else if (kind === "off") classes.push("is-off");
  els.dot.className = classes.join(" ");
  els.dot.title = label || "";
}

function setStats(text) {
  els.stats.textContent = text || "";
}

function showOverlay(title, detail, { spinner = false, retry = false } = {}) {
  els.overlay.classList.remove("hidden");
  els.overlayTitle.textContent = title;
  els.overlayDetail.textContent = detail || "";
  els.overlaySpinner.classList.toggle("hidden", !spinner);
  els.overlayRetry.classList.toggle("hidden", !retry);
}

function hideOverlay() {
  els.overlay.classList.add("hidden");
}

function icon(id) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#${id}`);
  svg.append(use);
  return svg;
}

function renderActions(actions) {
  for (const action of actions) {
    let control;
    if (action.type === "select") {
      control = document.createElement("select");
      control.setAttribute("aria-label", action.label);
      for (const option of action.options) {
        const node = document.createElement("option");
        node.value = option.value;
        node.textContent = option.label;
        if (option.value === action.value) node.selected = true;
        control.append(node);
      }
      control.addEventListener("change", () => action.onChange(control.value));
    } else {
      control = document.createElement("button");
      control.type = "button";
      control.className = "btn btn-secondary";
      control.append(icon(action.icon));
      const label = document.createElement("span");
      label.className = "btn-label";
      label.textContent = action.label;
      control.append(label);
      if (action.active) control.setAttribute("aria-pressed", "true");
      control.addEventListener("click", async () => {
        try {
          const next = await action.run();
          if (typeof next === "boolean") control.setAttribute("aria-pressed", String(next));
        } catch (error) {
          toast(error.message, "error");
        }
      });
    }
    els.actions.insertBefore(control, els.fullscreen);
  }
}

const ui = {
  setStatus,
  setStats,
  toast,
  showOverlay,
  hideOverlay,
};

function fail(title, detail) {
  setStatus("off", "Unavailable");
  showOverlay(title, detail, { spinner: false, retry: true });
}

async function boot() {
  const id = new URLSearchParams(location.search).get("host") || "";
  if (!/^[0-9a-fA-F-]{36}$/.test(id)) {
    fail("Invalid device link", "Open a device from the dashboard instead.");
    return;
  }

  setStatus("warn", "Connecting");
  showOverlay("Connecting…", "Opening a channel to your device.", { spinner: true });
  document.title = "Connecting… · Sethu";

  let session;
  try {
    session = await fetch("/api/session", { credentials: "same-origin" }).then((response) =>
      response.json(),
    );
  } catch {
    fail("Cannot reach the Sethu server", "Check your connection, then reconnect.");
    return;
  }
  if (!session.user) {
    location.replace("/");
    return;
  }

  let host;
  try {
    const response = await fetch(`/api/hosts/${id}`, { credentials: "same-origin" });
    if (!response.ok) {
      const body = await response.json().catch(() => null);
      throw new Error(body?.error?.message || `Could not load this device (${response.status}).`);
    }
    host = await response.json();
  } catch (error) {
    fail("Device unavailable", error.message);
    return;
  }

  document.title = `${host.name} · Sethu`;
  els.name.textContent = host.name;
  els.target.textContent = host.target;
  els.target.title = host.target;
  document.body.dataset.kind = host.kind;

  const mount =
    host.kind === "ios"
      ? mountIos
      : host.kind === "android"
        ? mountAndroid
        : mountVnc;
  try {
    activeViewer = await mount({
      host,
      screen: els.screen,
      ui,
      vendorNovnc: Boolean(session.novnc_vendored),
    });
    renderActions(activeViewer.actions || []);
  } catch (error) {
    fail("Could not start the session", error.message);
  }
}

els.overlayRetry.addEventListener("click", () => location.reload());

els.disconnect.addEventListener("click", () => {
  activeViewer?.destroy();
  activeViewer = null;
  setStatus("off", "Disconnected");
  setStats("");
  showOverlay("Session ended", "Reconnect when you are ready.", { retry: true });
});

els.fullscreen.addEventListener("click", async () => {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await els.stage.requestFullscreen();
  } catch {
    toast("Fullscreen is not available in this browser.", "error");
  }
});

window.addEventListener("beforeunload", () => activeViewer?.destroy());

boot();
