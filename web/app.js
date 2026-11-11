/* Sethu dashboard: auth, device list and the add-device flow. */

const $ = (selector, root = document) => root.querySelector(selector);

const KIND_META = {
  macos: { icon: "i-mac", label: "Mac · Screen Sharing" },
  windows: { icon: "i-windows", label: "Windows · VNC" },
  ios: { icon: "i-iphone", label: "iPhone / iPad · WDA" },
  android: { icon: "i-android", label: "Android · adb" },
};

function kindMeta(kind) {
  return KIND_META[kind] || KIND_META.macos;
}

const state = {
  user: null,
  csrf: "",
  hosts: [],
  files: [],
  registration: true,
  version: "",
  tab: "login",
  filter: "all",
  view: "devices",
  authOpen: false,
  usernameOnly: false,
  maxFileBytes: 0,
};

class ApiError extends Error {
  constructor(message, status = 0) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function api(path, { method = "GET", body } = {}) {
  const headers = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (method !== "GET" && method !== "HEAD" && state.csrf) {
    headers["x-sethu-csrf"] = state.csrf;
  }

  let response;
  try {
    response = await fetch(path, {
      method,
      headers,
      credentials: "same-origin",
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError("Cannot reach the Sethu server. Check your connection.", 0);
  }

  const contentType = response.headers.get("content-type") || "";
  const data = contentType.includes("application/json")
    ? await response.json().catch(() => null)
    : null;

  if (!response.ok) {
    const message = data?.error?.message || `Request failed (${response.status})`;
    throw new ApiError(message, response.status);
  }
  return data;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function icon(id, className = "icon") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", className);
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#${id}`);
  svg.append(use);
  return svg;
}

// ----- toasts ------------------------------------------------------------

function toast(message, kind = "info") {
  const container = $("#toasts");
  const node = el("div", `toast${kind === "error" ? " is-error" : kind === "success" ? " is-success" : ""}`, message);
  container.append(node);
  while (container.children.length > 3) container.firstElementChild.remove();
  window.setTimeout(() => {
    node.classList.add("is-leaving");
    window.setTimeout(() => node.remove(), 200);
  }, kind === "error" ? 6000 : 3600);
}

// ----- auth view ---------------------------------------------------------

const authForm = $("#auth-form");
const authEmail = $("#auth-email");
const authError = $("#auth-error");
const authSubmit = $("#auth-submit");
const authTabsSlot = $("#auth-tabs-slot");
const authPasswordSlot = $("#auth-password-slot");

const passwordInput = () => $("#auth-password");

function secureUiInstalled() {
  return Boolean($("#auth-tabs"));
}

function installSecureUi() {
  if (secureUiInstalled()) return;
  authTabsSlot.replaceChildren($("#auth-tabs-template").content.cloneNode(true));
  authPasswordSlot.replaceChildren($("#auth-password-template").content.cloneNode(true));
  const registerButton = document.querySelector('.tab[data-tab="register"]');
  if (registerButton) registerButton.classList.toggle("hidden", !state.registration);
}

function removeSecureUi() {
  authTabsSlot.replaceChildren();
  authPasswordSlot.replaceChildren();
}

authTabsSlot.addEventListener("click", (event) => {
  const button = event.target.closest(".tab");
  if (button) setTab(button.dataset.tab);
});

function setTab(tab) {
  if (!secureUiInstalled()) return;
  state.tab = tab === "register" && state.registration ? "register" : "login";
  document.querySelectorAll(".tab").forEach((button) => {
    const selected = button.dataset.tab === state.tab;
    button.classList.toggle("is-active", selected);
    button.setAttribute("aria-selected", String(selected));
  });

  const registering = state.tab === "register";
  $("#auth-heading").textContent = registering ? "Create your account" : "Sign in";
  $("#auth-subtitle").textContent = registering
    ? "This account owns the Sethu server."
    : "Welcome back. Pick up where you left off.";
  authSubmit.textContent = registering ? "Create account" : "Sign in";
  const password = passwordInput();
  if (password) {
    password.setAttribute("autocomplete", registering ? "new-password" : "current-password");
  }
  $("#auth-hint").textContent = registering
    ? "Use at least 10 characters. There is no password reset — store it safely."
    : "First run? Create the account that owns this server.";
  authError.textContent = "";
}

function configureAuthMode() {
  const usernameMode = state.usernameOnly;
  if (usernameMode) removeSecureUi();
  else installSecureUi();

  authEmail.type = usernameMode ? "text" : "email";
  authEmail.autocomplete = usernameMode ? "username" : "email";
  authEmail.placeholder = usernameMode ? "your-name" : "you@example.com";
  const label = authEmail.closest(".field")?.querySelector("span");
  if (label) label.textContent = usernameMode ? "Username" : "Email";

  $("#auth-footnote").textContent = usernameMode
    ? "Username-only access does not verify identity: anyone who reaches this server and knows a username can open that workspace. Keep Sethu on a LAN or VPN."
    : "Sessions use an HttpOnly, SameSite=Strict cookie. Run Sethu behind TLS before exposing it to the internet.";

  if (usernameMode) {
    state.tab = "login";
    $("#auth-heading").textContent = "Enter your username";
    $("#auth-subtitle").textContent = "No password. Your username opens its own devices and files.";
    authSubmit.textContent = "Continue";
    $("#auth-hint").textContent = "New username? An empty workspace is created automatically.";
    authError.textContent = "";
  } else {
    setTab(state.tab);
  }
}

authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  authError.textContent = "";

  if (state.usernameOnly) {
    const username = authEmail.value.trim();
    if (!username) {
      authError.textContent = "Enter a username.";
      authEmail.focus();
      return;
    }
    authSubmit.disabled = true;
    authSubmit.textContent = "Opening…";
    try {
      await api("/api/auth/login", {
        method: "POST",
        body: { username },
      });
      await loadSession();
      authForm.reset();
      await enterDashboard();
      toast(`Welcome, ${username}.`, "success");
    } catch (error) {
      authError.textContent = error.message;
      authEmail.focus();
    } finally {
      authSubmit.disabled = false;
      authSubmit.textContent = "Continue";
    }
    return;
  }

  const email = authEmail.value.trim();
  const password = passwordInput()?.value || "";
  if (!email || !password) {
    authError.textContent = "Enter your email and password.";
    return;
  }

  const registering = state.tab === "register";
  authSubmit.disabled = true;
  authSubmit.textContent = registering ? "Creating account…" : "Signing in…";

  try {
    await api(registering ? "/api/auth/register" : "/api/auth/login", {
      method: "POST",
      body: { email, password },
    });
    await loadSession();
    authForm.reset();
    await enterDashboard();
    toast(registering ? "Account created. Welcome to Sethu." : "Welcome back.", "success");
  } catch (error) {
    authError.textContent = error.message;
    passwordInput()?.select();
  } finally {
    authSubmit.disabled = false;
    authSubmit.textContent = state.tab === "register" ? "Create account" : "Sign in";
  }
});

$("#logout").addEventListener("click", async () => {
  try {
    await api("/api/auth/logout", { method: "POST" });
  } catch {
    /* Signing out locally is still the right outcome. */
  }
  state.user = null;
  state.csrf = "";
  state.hosts = [];
  state.files = [];
  state.view = "devices";
  state.filter = "all";
  showAuth();
  toast("Signed out.");
});

// ----- views -------------------------------------------------------------

function showAuth() {
  $("#dashboard-view").classList.add("hidden");
  $("#auth-view").classList.remove("hidden");
  $("#logout").classList.add("hidden");
  if (state.authOpen) {
    $("#auth-view").classList.add("hidden");
    $("#dashboard-view").classList.remove("hidden");
    return;
  }
  $("#auth-hint").classList.toggle("hidden", !state.registration && !state.usernameOnly);
  configureAuthMode();
  if (!state.registration && !state.usernameOnly) setTab("login");
  document.title = state.usernameOnly ? "Sethu — enter username" : "Sethu — sign in";
}

async function enterDashboard() {
  $("#auth-view").classList.add("hidden");
  $("#dashboard-view").classList.remove("hidden");
  $("#logout").classList.toggle("hidden", state.authOpen);
  document.title = "Sethu — your devices";
  setView("devices");
  await loadHosts();
}

function setView(view) {
  const filesView = view === "files";
  state.view = filesView ? "files" : "devices";
  $("#devices-panel").classList.toggle("hidden", filesView);
  $("#files-panel").classList.toggle("hidden", !filesView);
  $("#add-host").classList.toggle("hidden", filesView);
  document.querySelectorAll(".workspace-tab").forEach((button) => {
    const active = button.dataset.view === state.view;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
  });
  $("#dashboard-heading").textContent = filesView ? "File sharing" : "Your devices";
  $("#dashboard-note").textContent = filesView
    ? "Move any file between this MacBook and your Android, Windows, iPhone or iPad devices — then download it wherever you need it."
    : "Open a device, or add a Mac, Windows PC, iPhone, iPad or Android device reachable from this server.";
  if (filesView) loadFiles();
}

document.querySelectorAll(".workspace-tab").forEach((button) => {
  button.addEventListener("click", () => setView(button.dataset.view));
});

async function loadSession() {
  const info = await api("/api/session");
  state.user = info.user;
  state.csrf = info.csrf || "";
  state.registration = Boolean(info.registration);
  state.version = info.version || "";
  state.authOpen = Boolean(info.auth_open);
  state.usernameOnly = Boolean(info.username_only);
  state.maxFileBytes = Number(info.max_file_bytes) || 0;
  $("#version").textContent = state.version ? `v${state.version}` : "";
  $("#footer-version").textContent = state.version ? `v${state.version}` : "";
  return info;
}

// ----- device list -------------------------------------------------------

async function loadHosts() {
  try {
    state.hosts = (await api("/api/hosts")) || [];
    renderHosts();
  } catch (error) {
    if (error.status === 401) {
      state.user = null;
      showAuth();
      return;
    }
    toast(error.message, "error");
  }
}

function renderHosts() {
  const grid = $("#hosts-grid");
  const empty = $("#hosts-empty");
  const filters = $("#host-filters");
  const filterEmpty = $("#filter-empty");
  grid.replaceChildren();
  syncFilterTabs();

  filters.classList.toggle("hidden", state.hosts.length === 0);
  if (state.hosts.length === 0) {
    empty.classList.remove("hidden");
    filterEmpty.classList.add("hidden");
    return;
  }
  empty.classList.add("hidden");

  const visible =
    state.filter === "all"
      ? state.hosts
      : state.hosts.filter((host) => host.kind === state.filter);
  filterEmpty.classList.toggle("hidden", visible.length > 0);

  for (const host of visible) {
    grid.append(buildHostCard(host));
  }
}

function setFilter(filter) {
  state.filter = filter;
  renderHosts();
}

function syncFilterTabs() {
  document.querySelectorAll(".filter-tab").forEach((button) => {
    const active = button.dataset.filter === state.filter;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
  });
}

$("#host-filters").addEventListener("click", (event) => {
  const button = event.target.closest(".filter-tab");
  if (button) setFilter(button.dataset.filter);
});

function buildHostCard(host) {
  const meta = kindMeta(host.kind);
  const card = el("article", "host-card");
  card.dataset.kind = host.kind;
  card.dataset.hostId = host.id;

  const head = el("div", "host-card-head");
  const iconWrap = el("div", "host-icon");
  iconWrap.append(icon(meta.icon));

  const titles = el("div", "host-titles");
  titles.append(el("h3", "host-name", host.name), el("p", "host-kind", meta.label));

  const remove = el("button", "icon-btn danger");
  remove.type = "button";
  remove.dataset.action = "delete";
  remove.title = "Remove device";
  remove.setAttribute("aria-label", `Remove ${host.name}`);
  remove.append(icon("i-trash"));

  const pair = el("button", "icon-btn");
  pair.type = "button";
  pair.dataset.action = "pair";
  pair.title = "Pair a phone by QR";
  pair.setAttribute("aria-label", `Pair a phone to ${host.name}`);
  pair.append(icon("i-qr"));

  head.append(iconWrap, titles, pair, remove);

  const metaRow = el("div", "host-meta");
  const target = el("p", "host-target", host.target);
  target.title = host.target;
  metaRow.append(target);

  card.append(head, metaRow);
  if (host.notes) card.append(el("p", "host-notes", host.notes));

  const actions = el("div", "host-card-actions");
  const connect = el("a", "btn btn-primary");
  connect.href = `/viewer.html?host=${encodeURIComponent(host.id)}`;
  connect.append(icon("i-expand"), el("span", "btn-label", "Connect"));

  const test = el("button", "btn btn-secondary");
  test.type = "button";
  test.dataset.action = "test";
  test.append(el("span", "btn-label", "Test"));

  actions.append(connect, test);

  const result = el("div", "test-result hidden");
  result.dataset.role = "test-result";

  card.append(actions, result);
  return card;
}

$("#hosts-grid").addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const card = button.closest("[data-host-id]");
  if (!card) return;
  const host = state.hosts.find((item) => item.id === card.dataset.hostId);
  if (!host) return;

  if (button.dataset.action === "pair") {
    openPairDialog(host.id);
    return;
  }

  if (button.dataset.action === "test") {
    await runTest(host, card, button);
    return;
  }

  if (button.dataset.action === "delete") {
    if (!window.confirm(`Remove “${host.name}”? Viewers using it will stop working.`)) return;
    button.disabled = true;
    try {
      await api(`/api/hosts/${host.id}`, { method: "DELETE" });
      state.hosts = state.hosts.filter((item) => item.id !== host.id);
      renderHosts();
      toast(`${host.name} removed.`);
    } catch (error) {
      button.disabled = false;
      toast(error.message, "error");
    }
  }
});

async function runTest(host, card, button) {
  const result = $('[data-role="test-result"]', card);
  result.classList.remove("hidden", "is-ok", "is-error");
  result.textContent = "Testing the connection…";
  button.disabled = true;

  try {
    const data = await api(`/api/hosts/${host.id}/test`, { method: "POST" });
    result.textContent = `${data.ok ? "Reachable" : "Not reachable"} · ${data.latency_ms} ms — ${data.message}`;
    result.classList.add(data.ok ? "is-ok" : "is-error");
  } catch (error) {
    result.textContent = error.message;
    result.classList.add("is-error");
  } finally {
    button.disabled = false;
  }
}

// ----- add device dialog -------------------------------------------------

const dialog = $("#host-dialog");
const hostForm = $("#host-form");
const hostName = $("#host-name");
const hostTarget = $("#host-target");
const hostNotes = $("#host-notes");
const hostError = $("#host-error");
const hostSubmit = $("#host-submit");

function selectedKind() {
  const checked = hostForm.querySelector('input[name="kind"]:checked');
  return checked ? checked.value : "macos";
}

function applyKind(kind) {
  const config = {
    macos: {
      label: "Mac address",
      placeholder: "studio-mac.local:5900",
      hint: "Hostname or IP of the Mac. Port defaults to 5900.",
    },
    windows: {
      label: "PC address",
      placeholder: "desktop-pc.local:5900",
      hint: "Hostname or IP of the Windows PC running a VNC server. Port defaults to 5900.",
    },
    ios: {
      label: "WebDriverAgent URL",
      placeholder: "http://192.168.1.20:8100",
      hint: "Base URL of WebDriverAgent, usually port 8100 on the Mac it is tethered to.",
    },
    android: {
      label: "ADB serial",
      placeholder: "R58M1234567 or 192.168.1.30:5555",
      hint: "The serial shown by `adb devices` on the Sethu host. Enable USB or wireless debugging.",
    },
  }[kind] || {
    label: "Address",
    placeholder: "",
    hint: "",
  };

  $("#host-target-label").textContent = config.label;
  hostTarget.placeholder = config.placeholder;
  $("#host-target-hint").textContent = config.hint;
}

function openDialog() {
  hostForm.reset();
  hostError.textContent = "";
  applyKind("macos");
  if (typeof dialog.showModal === "function") dialog.showModal();
  else dialog.setAttribute("open", "");
  window.setTimeout(() => hostName.focus(), 60);
}

$("#add-host").addEventListener("click", openDialog);
$("#empty-add").addEventListener("click", openDialog);
$("#host-close").addEventListener("click", () => dialog.close());
$("#host-cancel").addEventListener("click", () => dialog.close());
dialog.addEventListener("click", (event) => {
  if (event.target === dialog) dialog.close();
});

hostForm.querySelectorAll('input[name="kind"]').forEach((input) => {
  input.addEventListener("change", () => applyKind(selectedKind()));
});

hostForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  hostError.textContent = "";

  const name = hostName.value.trim();
  const target = hostTarget.value.trim();
  const kind = selectedKind();
  const notes = hostNotes.value.trim();

  if (!name) {
    hostError.textContent = "Give this device a name.";
    hostName.focus();
    return;
  }
  if (!target) {
    hostError.textContent = kind === "ios" ? "Enter the WebDriverAgent URL." : "Enter the Mac's hostname or IP.";
    hostTarget.focus();
    return;
  }

  hostSubmit.disabled = true;
  hostSubmit.textContent = "Adding…";
  try {
    await api("/api/hosts", {
      method: "POST",
      body: { name, kind, target, notes: notes || null },
    });
    dialog.close();
    state.filter = "all";
    await loadHosts();
    toast(`${name} added.`, "success");
  } catch (error) {
    hostError.textContent = error.message;
  } finally {
    hostSubmit.disabled = false;
    hostSubmit.textContent = "Add device";
  }
});

// ----- shared files ------------------------------------------------------

const fileInput = $("#file-input");
const dropzone = $("#dropzone");
const uploadProgress = $("#upload-progress");
const filesList = $("#files-list");
let uploading = false;

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let scaled = value;
  let unit = -1;
  do {
    scaled /= 1024;
    unit += 1;
  } while (scaled >= 1024 && unit < units.length - 1);
  return `${scaled >= 10 ? Math.round(scaled) : scaled.toFixed(1)} ${units[unit]}`;
}

function formatDate(unix) {
  return dateFormatter.format(new Date(Number(unix) * 1000));
}

async function loadFiles() {
  try {
    state.files = (await api("/api/files")) || [];
    renderFiles();
  } catch (error) {
    if (error.status === 401) {
      showAuth();
      return;
    }
    toast(error.message, "error");
  }
}

function renderFiles() {
  filesList.replaceChildren();
  const total = state.files.reduce((sum, file) => sum + (Number(file.size) || 0), 0);
  $("#files-total").textContent = state.files.length
    ? `${state.files.length} file${state.files.length === 1 ? "" : "s"} · ${formatBytes(total)}`
    : "";
  $("#files-empty").classList.toggle("hidden", state.files.length !== 0);

  for (const file of state.files) {
    const row = el("div", "file-row");
    row.dataset.fileId = file.id;

    const fileIcon = el("div", "file-icon");
    fileIcon.append(icon("i-file"));

    const main = el("div", "file-main");
    const name = el("p", "file-name", file.name);
    name.title = file.name;
    main.append(name, el("p", "file-meta", `${formatBytes(file.size)} · ${formatDate(file.created_at)}`));

    const download = el("a", "icon-btn");
    download.href = `/api/files/${encodeURIComponent(file.id)}/download`;
    download.setAttribute("download", file.name);
    download.title = "Download";
    download.setAttribute("aria-label", `Download ${file.name}`);
    download.append(icon("i-download"));

    const share = el("button", "icon-btn");
    share.type = "button";
    share.dataset.action = "share-file";
    share.title = "Share";
    share.setAttribute("aria-label", `Share ${file.name}`);
    share.append(icon("i-share"));

    const remove = el("button", "icon-btn danger");
    remove.type = "button";
    remove.dataset.action = "delete-file";
    remove.title = "Delete";
    remove.setAttribute("aria-label", `Delete ${file.name}`);
    remove.append(icon("i-trash"));

    row.append(fileIcon, main, share, download, remove);
    filesList.append(row);
  }
}

const shareDialog = $("#share-dialog");

function openShareDialog(file) {  $("#share-file-name").textContent = `${file.name} · ${formatBytes(file.size)}`;
  $("#share-link").value = `${location.origin}/api/files/${encodeURIComponent(file.id)}/download`;

  const devices = $("#share-android-devices");
  devices.replaceChildren();
  const androidHosts = state.hosts.filter((host) => host.kind === "android");
  $("#share-android-section").classList.toggle("hidden", androidHosts.length === 0);
  for (const host of androidHosts) {
    const button = el("button", "btn btn-secondary btn-sm", `Send to ${host.name}`);
    button.type = "button";
    button.addEventListener("click", () => sendToAndroid(file, host, button));
    devices.append(button);
  }

  if (typeof shareDialog.showModal === "function") shareDialog.showModal();
  else shareDialog.setAttribute("open", "");
}

async function sendToAndroid(file, host, button) {
  const label = button.textContent;
  button.disabled = true;
  button.textContent = "Sending…";
  try {
    const result = await api(`/api/files/${file.id}/send`, {
      method: "POST",
      body: { host_id: host.id },
    });
    toast(result.message || `Sent to ${host.name}.`, "success");
  } catch (error) {
    toast(error.message, "error");
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
}

$("#copy-share-link").addEventListener("click", async () => {
  const input = $("#share-link");
  try {
    await navigator.clipboard.writeText(input.value);
    toast("Download link copied.", "success");
  } catch {
    input.select();
    try {
      document.execCommand("copy");
      toast("Download link copied.", "success");
    } catch {
      toast("Could not copy — select the link and copy it manually.", "error");
    }
  }
});

filesList.addEventListener("click", async (event) => {
  const row = event.target.closest("[data-file-id]");
  if (!row) return;
  const file = state.files.find((item) => item.id === row.dataset.fileId);
  if (!file) return;

  if (event.target.closest('button[data-action="share-file"]')) {
    openShareDialog(file);
    return;
  }

  const button = event.target.closest('button[data-action="delete-file"]');
  if (!button) return;
  if (!window.confirm(`Delete “${file.name}”?`)) return;

  button.disabled = true;
  try {
    await api(`/api/files/${file.id}`, { method: "DELETE" });
    state.files = state.files.filter((item) => item.id !== file.id);
    renderFiles();
    toast("File deleted.");
  } catch (error) {
    button.disabled = false;
    toast(error.message, "error");
  }
});

function showProgress(name, fraction, index, total) {
  uploadProgress.classList.remove("hidden");
  $("#upload-label").textContent = `Uploading ${index}/${total} · ${name}`;
  const percent = Math.max(0, Math.min(100, Math.round(fraction * 100)));
  $("#upload-percent").textContent = `${percent}%`;
  $("#upload-bar").style.width = `${percent}%`;
}

function hideProgress() {
  $("#upload-bar").style.width = "0%";
  uploadProgress.classList.add("hidden");
}

function uploadOne(file, index, total) {
  return new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/files");
    if (state.csrf) xhr.setRequestHeader("x-sethu-csrf", state.csrf);

    xhr.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable) {
        showProgress(file.name, event.loaded / event.total, index, total);
      }
    });

    xhr.addEventListener("load", () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        toast(`${file.name} uploaded.`, "success");
      } else {
        let message = `upload failed (${xhr.status})`;
        try {
          message = JSON.parse(xhr.responseText).error.message || message;
        } catch {
          /* non-JSON error */
        }
        toast(`${file.name}: ${message}`, "error");
      }
      resolve();
    });

    xhr.addEventListener("error", () => {
      toast(`${file.name}: upload failed. Check the connection.`, "error");
      resolve();
    });

    const form = new FormData();
    form.append("file", file, file.name);
    showProgress(file.name, 0, index, total);
    xhr.send(form);
  });
}

async function uploadFiles(fileList) {
  const files = Array.from(fileList || []);
  if (!files.length || uploading) return;
  uploading = true;
  try {
    for (let index = 0; index < files.length; index += 1) {
      const file = files[index];
      if (state.maxFileBytes && file.size > state.maxFileBytes) {
        toast(`${file.name} is larger than the ${formatBytes(state.maxFileBytes)} limit.`, "error");
        continue;
      }
      await uploadOne(file, index + 1, files.length);
    }
  } finally {
    uploading = false;
    hideProgress();
    await loadFiles();
  }
}

dropzone.addEventListener("click", (event) => {
  if (event.target.closest("button")) return;
  fileInput.click();
});

$("#pick-files").addEventListener("click", (event) => {
  event.stopPropagation();
  fileInput.click();
});

dropzone.addEventListener("keydown", (event) => {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    fileInput.click();
  }
});

fileInput.addEventListener("change", () => {
  const files = Array.from(fileInput.files || []);
  fileInput.value = "";
  uploadFiles(files);
});

["dragenter", "dragover"].forEach((name) => {
  dropzone.addEventListener(name, (event) => {
    event.preventDefault();
    event.stopPropagation();
    dropzone.classList.add("is-dragging");
  });
});

["dragleave", "dragend"].forEach((name) => {
  dropzone.addEventListener(name, (event) => {
    event.preventDefault();
    dropzone.classList.remove("is-dragging");
  });
});

dropzone.addEventListener("drop", (event) => {
  event.preventDefault();
  event.stopPropagation();
  dropzone.classList.remove("is-dragging");
  uploadFiles(event.dataTransfer.files);
});

window.addEventListener("dragover", (event) => event.preventDefault());
window.addEventListener("drop", (event) => event.preventDefault());

// ----- QR pairing --------------------------------------------------------

const pairDialog = $("#pair-dialog");
let pairTimer = null;

function openPairDialog(hostId = null) {
  window.clearInterval(pairTimer);
  $("#pair-target").textContent = "Generating a one-time code…";
  $("#pair-qr").removeAttribute("src");
  $("#pair-url").value = "";
  $("#pair-expiry").textContent = "";

  if (typeof pairDialog.showModal === "function") pairDialog.showModal();
  else pairDialog.setAttribute("open", "");

  api("/api/pair", {
    method: "POST",
    body: hostId ? { host_id: hostId } : {},
  })
    .then((data) => {
      $("#pair-qr").src = data.qr;
      $("#pair-url").value = data.url;
      $("#pair-target").textContent = `Opens ${data.target} on the other device.`;
      let remaining = Number(data.expires_in) || 120;

      const update = () => {
        if (remaining <= 0) {
          $("#pair-expiry").textContent =
            "Code expired — close and reopen this window to generate a new one.";
          window.clearInterval(pairTimer);
          return;
        }
        const minutes = Math.floor(remaining / 60);
        const seconds = String(remaining % 60).padStart(2, "0");
        $("#pair-expiry").textContent = `Code expires in ${minutes}:${seconds}.`;
        remaining -= 1;
      };

      update();
      pairTimer = window.setInterval(update, 1000);
    })
    .catch((error) => {
      toast(error.message, "error");
      pairDialog.close();
    });
}

$("#pair-mobile").addEventListener("click", () => openPairDialog());

pairDialog.addEventListener("close", () => {
  window.clearInterval(pairTimer);
  $("#pair-qr").removeAttribute("src");
  $("#pair-url").value = "";
});

$("#copy-pair-url").addEventListener("click", async () => {
  const input = $("#pair-url");
  try {
    await navigator.clipboard.writeText(input.value);
    toast("Pairing link copied.", "success");
  } catch {
    input.select();
    try {
      document.execCommand("copy");
      toast("Pairing link copied.", "success");
    } catch {
      toast("Could not copy — select the link manually.", "error");
    }
  }
});

// ----- in-app QR scanner ------------------------------------------------

const scanDialog = $("#scan-dialog");
const scanVideo = $("#scan-video");
const scanWorkCanvas = document.createElement("canvas");
let scanStream = null;
let scanFrameRequest = null;

function isMobileOrTablet() {
  const ua = navigator.userAgent || "";
  const data = navigator.userAgentData;
  if (data && data.mobile) return true;
  if (/android|iphone|ipod|ipad|tablet|silk|kindle|playbook|mobile/i.test(ua)) {
    return true;
  }
  // iPadOS reports a Macintosh user agent but exposes touch points.
  if (/macintosh/i.test(ua) && navigator.maxTouchPoints > 1) return true;
  return false;
}

function applyDeviceUi() {
  const mobile = isMobileOrTablet();
  document.querySelectorAll(".phone-only").forEach((element) => {
    element.classList.toggle("hidden", !mobile);
  });
}

function openScanDialog() {
  if (!isMobileOrTablet()) {
    toast("QR scanning is available on phones and tablets. Show the QR here and scan it with your phone.", "error");
    return;
  }
  $("#scan-status").textContent = window.isSecureContext
    ? "Point the camera at the QR code."
    : "Camera scanning needs HTTPS. Take a photo of the QR or paste the link.";
  $("#scan-manual").classList.add("hidden");
  $("#scan-url").value = "";
  $("#scan-photo").value = "";

  if (typeof scanDialog.showModal === "function") scanDialog.showModal();
  else scanDialog.setAttribute("open", "");

  startScanCamera();
}

async function startScanCamera() {
  if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia) {
    $("#scan-live-wrap").classList.add("hidden");
    return;
  }
  try {
    scanStream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: { facingMode: { ideal: "environment" } },
    });
    scanVideo.srcObject = scanStream;
    $("#scan-live-wrap").classList.remove("hidden");
    await scanVideo.play();
    scanLoop();
  } catch {
    $("#scan-live-wrap").classList.add("hidden");
    $("#scan-status").textContent =
      "Camera unavailable. Take a photo of the QR or paste the link.";
  }
}

function stopScanCamera() {
  if (scanFrameRequest) window.cancelAnimationFrame(scanFrameRequest);
  scanFrameRequest = null;
  if (scanStream) {
    for (const track of scanStream.getTracks()) track.stop();
    scanStream = null;
  }
  scanVideo.srcObject = null;
}

function scanLoop() {
  if (!scanDialog.open) return;
  if (window.jsQR && scanVideo.readyState >= 2 && scanVideo.videoWidth > 0) {
    scanWorkCanvas.width = scanVideo.videoWidth;
    scanWorkCanvas.height = scanVideo.videoHeight;
    const context = scanWorkCanvas.getContext("2d", { willReadFrequently: true });
    context.drawImage(scanVideo, 0, 0);
    const image = context.getImageData(0, 0, scanWorkCanvas.width, scanWorkCanvas.height);
    const code = window.jsQR(image.data, image.width, image.height, {
      inversionAttempts: "dontInvert",
    });
    if (code && code.data) {
      handleScanResult(code.data);
      return;
    }
  }
  scanFrameRequest = window.requestAnimationFrame(scanLoop);
}

async function scanPhoto(file) {
  if (!window.jsQR) {
    $("#scan-status").textContent = "The QR decoder did not load. Reload and try again.";
    return;
  }
  const bitmap = await createImageBitmap(file).catch(() => null);
  if (!bitmap) {
    $("#scan-status").textContent = "That image could not be read. Try another photo.";
    return;
  }
  const scale = Math.min(1, 1600 / Math.max(bitmap.width, bitmap.height));
  scanWorkCanvas.width = Math.max(1, Math.round(bitmap.width * scale));
  scanWorkCanvas.height = Math.max(1, Math.round(bitmap.height * scale));
  const context = scanWorkCanvas.getContext("2d", { willReadFrequently: true });
  context.drawImage(bitmap, 0, 0, scanWorkCanvas.width, scanWorkCanvas.height);
  bitmap.close();

  const image = context.getImageData(0, 0, scanWorkCanvas.width, scanWorkCanvas.height);
  const code = window.jsQR(image.data, image.width, image.height, {
    inversionAttempts: "attemptBoth",
  });
  if (!code || !code.data) {
    $("#scan-status").textContent =
      "No QR code found. Make sure the whole code is visible and well lit.";
    return;
  }
  handleScanResult(code.data);
}

function handleScanResult(raw) {
  let url;
  try {
    url = new URL(String(raw).trim());
  } catch {
    $("#scan-status").textContent = "That QR code does not contain a link.";
    return;
  }
  if (!["http:", "https:"].includes(url.protocol) || !url.pathname.startsWith("/pair/")) {
    $("#scan-status").textContent = "That is not a Sethu pairing code.";
    return;
  }
  if (
    url.origin !== location.origin &&
    !window.confirm(`This code opens ${url.origin}. Continue?`)
  ) {
    return;
  }
  stopScanCamera();
  scanDialog.close();
  window.location.href = url.href;
}

$("#scan-auth").addEventListener("click", openScanDialog);
$("#scan-qr").addEventListener("click", openScanDialog);
$("#host-scan").addEventListener("click", () => {
  dialog.close();
  openScanDialog();
});

$("#scan-photo").addEventListener("change", () => {
  const file = $("#scan-photo").files?.[0];
  if (file) scanPhoto(file);
});

$("#scan-paste-toggle").addEventListener("click", () => {
  $("#scan-manual").classList.toggle("hidden");
  $("#scan-url").focus();
});

$("#scan-go").addEventListener("click", () => {
  const value = $("#scan-url").value.trim();
  if (value) handleScanResult(value);
});

$("#scan-url").addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    const value = $("#scan-url").value.trim();
    if (value) handleScanResult(value);
  }
});

scanDialog.addEventListener("close", stopScanCamera);

// ----- boot --------------------------------------------------------------

async function boot() {
  applyDeviceUi();
  try {
    const info = await loadSession();
    if (info.user) await enterDashboard();
    else showAuth();
  } catch (error) {
    showAuth();
    if (error.status !== 401) toast(error.message, "error");
  } finally {
    const boot = $("#boot");
    boot.style.opacity = "0";
    window.setTimeout(() => boot.remove(), 240);
  }
}

boot();
