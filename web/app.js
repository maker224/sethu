/* Sethu dashboard: auth, device list and the add-device flow. */

const $ = (selector, root = document) => root.querySelector(selector);

const state = {
  user: null,
  csrf: "",
  hosts: [],
  registration: true,
  version: "",
  tab: "login",
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
const authPassword = $("#auth-password");
const authError = $("#auth-error");
const authSubmit = $("#auth-submit");
const registerTab = $("#register-tab");

function setTab(tab) {
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
  authPassword.setAttribute("autocomplete", registering ? "new-password" : "current-password");
  $("#auth-hint").textContent = registering
    ? "Use at least 10 characters. There is no password reset — store it safely."
    : "First run? Create the account that owns this server.";
  authError.textContent = "";
}

registerTab.addEventListener("click", () => setTab("register"));
$('.tab[data-tab="login"]').addEventListener("click", () => setTab("login"));

authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  authError.textContent = "";

  const email = authEmail.value.trim();
  const password = authPassword.value;
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
    authPassword.select();
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
  showAuth();
  toast("Signed out.");
});

// ----- views -------------------------------------------------------------

function showAuth() {
  $("#dashboard-view").classList.add("hidden");
  $("#auth-view").classList.remove("hidden");
  $("#logout").classList.add("hidden");
  registerTab.classList.toggle("hidden", !state.registration);
  $("#auth-hint").classList.toggle("hidden", !state.registration);
  if (!state.registration) setTab("login");
  document.title = "Sethu — sign in";
}

async function enterDashboard() {
  $("#auth-view").classList.add("hidden");
  $("#dashboard-view").classList.remove("hidden");
  $("#logout").classList.remove("hidden");
  document.title = "Sethu — your devices";
  await loadHosts();
}

async function loadSession() {
  const info = await api("/api/session");
  state.user = info.user;
  state.csrf = info.csrf || "";
  state.registration = Boolean(info.registration);
  state.version = info.version || "";
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
  grid.replaceChildren();

  if (state.hosts.length === 0) {
    empty.classList.remove("hidden");
    return;
  }
  empty.classList.add("hidden");

  for (const host of state.hosts) {
    grid.append(buildHostCard(host));
  }
}

function buildHostCard(host) {
  const card = el("article", "host-card");
  card.dataset.kind = host.kind;
  card.dataset.hostId = host.id;

  const head = el("div", "host-card-head");
  const iconWrap = el("div", "host-icon");
  iconWrap.append(icon(host.kind === "ios" ? "i-iphone" : "i-mac"));

  const titles = el("div", "host-titles");
  titles.append(
    el("h3", "host-name", host.name),
    el("p", "host-kind", host.kind === "ios" ? "iPhone · WebDriverAgent" : "Mac · Screen Sharing"),
  );

  const remove = el("button", "icon-btn danger");
  remove.type = "button";
  remove.dataset.action = "delete";
  remove.title = "Remove device";
  remove.setAttribute("aria-label", `Remove ${host.name}`);
  remove.append(icon("i-trash"));

  head.append(iconWrap, titles, remove);

  const meta = el("div", "host-meta");
  const target = el("p", "host-target", host.target);
  target.title = host.target;
  meta.append(target);

  card.append(head, meta);
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
  const mac = kind !== "ios";
  $("#host-target-label").textContent = mac ? "Mac address" : "WebDriverAgent URL";
  hostTarget.placeholder = mac ? "studio-mac.local:5900" : "http://192.168.1.20:8100";
  $("#host-target-hint").textContent = mac
    ? "Hostname or IP of the Mac. Port defaults to 5900."
    : "Base URL of WebDriverAgent, usually port 8100 on the Mac it is tethered to.";
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
    await loadHosts();
    toast(`${name} added.`, "success");
  } catch (error) {
    hostError.textContent = error.message;
  } finally {
    hostSubmit.disabled = false;
    hostSubmit.textContent = "Add device";
  }
});

// ----- boot --------------------------------------------------------------

async function boot() {
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
