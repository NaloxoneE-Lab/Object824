// The board's whole job: ask the kernel, draw the answer.
//
// The rule from P3 still holds — no sentence about a machine is written here. What changed
// with the redesign is where those sentences sit: the stage shows one distribution at a
// time, the yellow control carries the next step the kernel named, and everything that used
// to be a list on the first screen is one click away in a sheet.
//
// The window's own furniture (rail, menu, settings) speaks from `o824-present`'s chrome
// catalog, in the language Windows is set to. A hard-coded label here would be a bug even
// when it is only one word.

const invoke =
  window.__TAURI__?.core?.invoke ??
  (async () => {
    throw new Error("This page runs inside O824.");
  });

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);

const ORDER = ["desktop", "web", "tui"];
const ART = {
  desktop: "assets/tab-desktop.png",
  web: "assets/tab-web.png",
  tui: "assets/tab-tui.png",
};
const SETTINGS_KEY = "o824.settings.v1";
const REPO = "https://github.com/NaloxoneE-Lab/Object824";

const state = {
  chrome: null,
  info: { version: "", tray: false, autostart: false, log_path: null },
  trust: null,
  labels: {},
  installs: [],
  byKind: { desktop: [], web: [], tui: [] },
  kindState: { desktop: "absent", web: "absent", tui: "absent" },
  kind: "desktop",
  selectedId: null,
  copy: { headline: "…", reason: "", next_step: "", action: "" },
  findings: [],
  diagnosedId: null,
  lastRepair: null,
  running: false,
  url: null,
  plan: null,
  update: null,
  busy: false,
  sheet: null,
  settings: { tray: true, autoupdate: false, afterStart: false, closeNotice: false },
};

// ---------------------------------------------------------------- vocabulary

const t = (key) => state.chrome?.[key] ?? "";
const fill = (sentence, values) =>
  sentence.replace(/\{(\w+)\}/g, (match, name) =>
    Object.prototype.hasOwnProperty.call(values, name) ? String(values[name]) : match,
  );

function formName(kind) {
  const row = state.byKind[kind]?.[0];
  return row?.name || t(`form_${kind}`) || kind;
}

function kindName(kind) {
  return t(`kind_${kind}`) || formName(kind);
}

function selectedInstall() {
  const list = state.byKind[state.kind] || [];
  return list.find((row) => row.id === state.selectedId) || list[0] || null;
}

// ------------------------------------------------------------------ settings

function loadSettings() {
  try {
    const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}");
    for (const key of ["tray", "autoupdate", "afterStart", "closeNotice"]) {
      if (typeof saved[key] === "boolean") state.settings[key] = saved[key];
    }
  } catch {
    /* a corrupt preference is a default preference */
  }
}

function saveSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(state.settings));
  } catch {
    /* the window still works with defaults */
  }
}

// --------------------------------------------------------------------- toast

let toastTimer = 0;

function toast(message, { error = false, timeout = 0 } = {}) {
  if (!message) return;
  const node = $("toast");
  $("toast-text").textContent = message;
  node.classList.toggle("error", error);
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    node.hidden = true;
  }, timeout || (error ? 7000 : 3400));
}

// -------------------------------------------------------------------- sheets

function openSheet(id, focusId) {
  closeMenu();
  for (const sheet of document.querySelectorAll(".sheet")) {
    sheet.hidden = sheet.id !== id;
  }
  $("overlay").hidden = false;
  state.sheet = id;
  const target = focusId ? $(focusId) : $(id)?.querySelector(".sheet-x");
  setTimeout(() => target?.focus(), 30);
}

function closeSheet() {
  $("overlay").hidden = true;
  state.sheet = null;
}

// --------------------------------------------------------------------- chrome

function applyChrome() {
  const c = state.chrome;
  const set = (id, text) => {
    const node = $(id);
    if (node) node.textContent = text;
  };

  set("update-label", c.update);
  set("settings-title", c.settings);
  set("nav-launcher", c.launcher_settings);
  set("nav-about", c.about);
  set("group-startup", c.startup);
  set("set-autostart-title", c.autostart);
  set("set-autostart-detail", c.autostart_detail);
  set("group-update", c.update_section);
  set("set-autoupdate-title", c.autoupdate);
  set("set-autoupdate-detail", c.autoupdate_detail);
  set("group-close", c.close_section);
  set("set-tray-title", c.tray);
  set("set-tray-detail", c.tray_detail);
  set("tray-note", c.tray_unavailable);
  set("about-version", `${c.version} ${state.info.version}`);
  set("about-tagline", c.tagline);
  set("about-repo", c.repo);
  set("about-update", c.update);
  set("about-trust", state.trust?.why || "");
  set("about-copy", c.copy);
  set("about-log", c.open_log);
  set("details-title", c.details);
  set("details-copy", c.copy);
  set("details-close", c.close);
  set("details-undo", state.labels.undo_label || c.details);
  set("update-title", c.update_title);
  set("update-cancel", c.update_later);
  set("install-title", c.install);
  set("install-close", c.close);
  set("close-title", c.close_notice_title);
  set("close-reason", c.close_notice_reason);
  set("close-remember-label", c.close_remember);
  set("close-keep", c.close_keep);
  set("close-quit", c.close_quit);
  set("installs-title", c.all_installs);
  set("after-start-label", c.after_start);
  set("detail-link", c.details);
  set("menu-settings", c.launcher_settings);
  set("menu-shortcut", c.shortcut);
  set("menu-integrity", c.integrity);
  set("menu-cache", c.cache);

  for (const id of ["settings", "minimize", "close", "update", "all-installs"]) {
    const label =
      id === "settings"
        ? c.settings
        : id === "minimize"
          ? c.minimize
          : id === "close"
            ? c.close
            : id === "update"
              ? c.update
              : c.all_installs;
    $(id)?.setAttribute("aria-label", label);
  }
  $("tabs").setAttribute("aria-label", c.distributions);
  $("menu-toggle").setAttribute("aria-label", c.more);
  for (const button of document.querySelectorAll(".sheet-x")) {
    button.setAttribute("aria-label", c.close);
  }
  $("about-log").hidden = !state.info.log_path;
}

// ---------------------------------------------------------------------- rail

function renderTabs() {
  $("tabs").replaceChildren(
    ...ORDER.map((kind) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "tab";
      button.dataset.kind = kind;
      button.dataset.state = state.kindState[kind];
      button.title = formName(kind);
      button.setAttribute("aria-label", formName(kind));

      const image = document.createElement("img");
      image.src = ART[kind];
      image.alt = "";
      const badge = document.createElement("span");
      badge.className = `badge ${state.kindState[kind]}`;

      button.append(image, badge);
      button.addEventListener("click", () => void select(kind));
      return button;
    }),
  );
  paintTabs();
}

function paintTabs() {
  for (const button of $("tabs").querySelectorAll(".tab")) {
    const kind = button.dataset.kind;
    button.dataset.state = state.kindState[kind];
    button.classList.toggle("is-active", kind === state.kind);
    const badge = button.querySelector(".badge");
    badge.className = `badge ${state.kindState[kind]}`;
  }
}

// -------------------------------------------------------------------- selection

let selectToken = 0;

async function select(kind, { id = null } = {}) {
  const token = ++selectToken;
  state.kind = kind;
  const list = state.byKind[kind] || [];
  const install = (id && list.find((row) => row.id === id)) || list[0] || null;
  state.selectedId = install?.id ?? null;
  state.running = false;
  state.url = null;
  $("open-link").hidden = true;
  paintTabs();
  closeMenu();

  if (!install) {
    state.findings = [];
    state.diagnosedId = null;
    state.kindState[kind] = "absent";
    paintTabs();
    const copy = await invoke("absent", { kind });
    if (token !== selectToken) return;
    setCopy({ ...copy, action: "install" });
    return;
  }

  state.findings = [];
  state.diagnosedId = null;
  setCopy({ headline: t("checking"), reason: "", next_step: "", action: "" });
  setBusy(true);
  try {
    const findings = await invoke("diagnose", { installId: install.id });
    if (token !== selectToken) return;
    state.findings = findings;
    state.diagnosedId = install.id;
    install.findings = findings.length;
    install.repairable = findings.filter((finding) => finding.repairable).length;
    state.kindState[kind] = findings.length ? "attention" : "ok";
    const copy = await invoke("summarize", {
      layout: null,
      installs: [
        {
          kind: install.kind,
          version: install.version,
          findings: findings.length,
          repairable: install.repairable,
        },
      ],
    });
    if (token !== selectToken) return;
    setCopy(copy);
    paintTabs();
  } catch (error) {
    if (token !== selectToken) return;
    state.kindState[kind] = "attention";
    paintTabs();
    setCopy({ headline: t("problems"), reason: String(error), next_step: "", action: "" });
    toast(String(error), { error: true });
  } finally {
    if (token === selectToken) setBusy(false);
  }
}

function setCopy(copy) {
  state.copy = copy;
  const headline = copy.headline || "…";
  $("headline").textContent = headline;
  $("headline").classList.toggle("long", headline.length > 24);
  $("reason").textContent = copy.reason || "";
  $("eyebrow-kind").textContent = kindName(state.kind);

  const install = selectedInstall();
  const versionText = install ? install.version : "";
  $("version").textContent = versionText;
  // The kernel's "ready" sentence already names the version; printing it twice in two
  // lines is the kind of small repetition that makes a screen look assembled.
  $("version").hidden = !install || headline.includes(versionText);

  $("state-dot").className = `state-dot ${dotClass()}`;
  // The pill never disappears: a launcher without its start control reads as broken. While
  // there is no next step it says what is happening and is disabled.
  $("primary").hidden = false;
  $("primary-label").textContent = copy.next_step || t("checking");
  $("primary").disabled = state.busy || !copy.next_step;
  $("cta-shell").classList.toggle("is-idle", !copy.next_step);
  setPrimaryIcon(copy.action || "start");
  $("detail-link").hidden = state.findings.length === 0;
}

function dotClass() {
  const kind = state.kindState[state.kind];
  if (kind === "attention") return "attention";
  if (kind === "ok") return "ok";
  return "";
}

const ICONS = {
  play: '<path d="M4.5 3.5v17L19.5 12z"/>',
  repair:
    '<path d="M14.6 6.4a4.2 4.2 0 0 0-5.5 5.3L4.2 16.6V20h3.4l4.9-4.9a4.2 4.2 0 0 0 5.3-5.5l-2.6 2.6-2.4-.7-.7-2.4z"/>',
  install:
    '<path d="M12 3.5v11"/><path d="m7.5 10 4.5 4.5 4.5-4.5"/><path d="M4.5 17.5v1A2.5 2.5 0 0 0 7 21h10a2.5 2.5 0 0 0 2.5-2.5v-1"/>',
  choose:
    '<rect x="4" y="4" width="6.5" height="6.5" rx="1"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1"/>',
};

function setPrimaryIcon(action) {
  const icon = $("primary-icon");
  const name = Object.prototype.hasOwnProperty.call(ICONS, action) ? action : "play";
  icon.innerHTML = ICONS[name];
  icon.classList.toggle("play", name === "play");
}

function setBusy(busy) {
  state.busy = busy;
  $("primary").disabled = busy || !state.copy.next_step;
  $("menu-toggle").disabled = busy;
}

// ------------------------------------------------------------------ actions

async function runForm(action) {
  const install = selectedInstall();
  if (!install) {
    await openInstall();
    return;
  }
  setBusy(true);
  toast(action === "start" ? t("starting") : t("stopping"));
  try {
    const result = await invoke("form_action", { installId: install.id, action });
    state.running = result.running;
    state.url = result.url || null;
    if (result.url) {
      $("open-link").hidden = false;
      $("open-link").textContent = t("open");
    }
    toast(
      action === "start"
        ? result.running
          ? t("started")
          : t("problems")
        : result.running
          ? t("already_running")
          : t("stopped"),
    );
    if (state.settings.afterStart && action === "start") {
      setTimeout(() => void select(state.kind, { id: install.id }), 1600);
    }
  } catch (error) {
    toast(String(error), { error: true });
  } finally {
    setBusy(false);
  }
}

async function askForPlan() {
  const install = selectedInstall();
  if (!install) return;
  setBusy(true);
  try {
    const plan = await invoke("repair_plan", { installId: install.id, action: "repair" });
    state.plan = { install, plan };
    $("plan-title").textContent = state.copy.next_step || t("details");
    $("plan-backup").textContent = plan.backup;
    $("plan-heading").textContent = plan.heading;
    $("plan-steps").replaceChildren(
      ...plan.steps.map((step) => {
        const item = document.createElement("li");
        item.textContent = step;
        return item;
      }),
    );
    $("plan-cancel").textContent = plan.cancel;
    $("plan-confirm").textContent = plan.confirm;
    openSheet("sheet-plan", "plan-cancel");
  } catch (error) {
    const text = String(error);
    toast(/nothing to repair/i.test(text) ? t("nothing_to_repair") : text, { error: true });
  } finally {
    setBusy(false);
  }
}

async function runRepair() {
  if (!state.plan) return;
  const { install } = state.plan;
  $("plan-confirm").disabled = true;
  try {
    const result = await invoke("repair_apply", { installId: install.id });
    closeSheet();
    state.lastRepair = install.id;
    toast(result.findings === 0 ? t("details_none") : `${t("problems")}: ${result.findings}`);
    await select(install.kind, { id: install.id });
  } catch (error) {
    closeSheet();
    toast(String(error), { error: true });
  } finally {
    $("plan-confirm").disabled = false;
    state.plan = null;
  }
}

async function runUndo() {
  const install = selectedInstall();
  if (!install) return;
  $("details-undo").disabled = true;
  try {
    const result = await invoke("undo_repair", { installId: install.id });
    closeSheet();
    state.lastRepair = null;
    toast(result.findings === 0 ? t("details_none") : `${t("problems")}: ${result.findings}`);
    await select(install.kind, { id: install.id });
  } catch (error) {
    toast(String(error), { error: true });
  } finally {
    $("details-undo").disabled = false;
  }
}

// ------------------------------------------------------------------- details

function renderFindings(findings) {
  if (findings.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.innerHTML =
      '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="m8 12.5 2.6 2.6L16.5 9"/></svg>';
    const line = document.createElement("div");
    line.textContent = t("details_none");
    empty.append(line);
    return [empty];
  }
  return findings.map((finding) => {
    const card = document.createElement("article");
    card.className = `finding${finding.repairable ? " repairable" : ""}`;
    const title = document.createElement("h3");
    title.textContent = finding.headline;
    const reason = document.createElement("p");
    reason.textContent = finding.reason;
    const next = document.createElement("p");
    next.className = "next";
    next.textContent = finding.next_step;
    card.append(title, reason, next);
    return card;
  });
}

async function openDetails() {
  const install = selectedInstall();
  if (!install) {
    await openInstall();
    return;
  }
  openSheet("sheet-details", "details-close");
  const body = $("details-body");
  body.replaceChildren();
  const spinner = document.createElement("div");
  spinner.className = "spinner";
  body.append(spinner);
  $("details-repair").hidden = true;
  $("details-undo").hidden = true;

  try {
    const findings =
      state.diagnosedId === install.id
        ? state.findings
        : await invoke("diagnose", { installId: install.id });
    state.findings = findings;
    state.diagnosedId = install.id;
    body.replaceChildren(...renderFindings(findings));
    const repairable = findings.filter((finding) => finding.repairable).length;
    $("details-repair").hidden = repairable === 0;
    $("details-repair").textContent = t("repair");
    $("details-undo").hidden = state.lastRepair !== install.id;
    $("detail-link").hidden = findings.length === 0;
  } catch (error) {
    body.textContent = String(error);
  }
}

function diagnosticText() {
  const install = selectedInstall();
  const lines = [];
  if (install) lines.push(`${install.name} ${install.version}`);
  if (state.copy.headline) lines.push(state.copy.headline);
  if (state.copy.reason) lines.push(state.copy.reason);
  for (const finding of state.findings) {
    lines.push(`- ${finding.headline}: ${finding.reason} → ${finding.next_step}`);
  }
  return lines.join("\n");
}

async function copyDiagnostics() {
  try {
    await navigator.clipboard.writeText(diagnosticText());
    toast(t("copied"));
  } catch (error) {
    toast(String(error), { error: true });
  }
}

// ------------------------------------------------------------------- installs

function openInstalls() {
  const body = $("installs-body");
  if (state.installs.length === 0) {
    body.textContent = t("details_none");
  } else {
    body.replaceChildren(
      ...state.installs.map((row) => {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "install-row";

        const name = document.createElement("span");
        name.className = "name";
        name.textContent = row.name;
        const meta = document.createElement("span");
        meta.className = "meta";
        meta.textContent = row.version;
        const path = document.createElement("span");
        path.className = "path";
        path.textContent = row.root;

        button.append(name, meta, path);
        button.addEventListener("click", () => {
          closeSheet();
          void select(row.kind, { id: row.id });
        });
        return button;
      }),
    );
  }
  openSheet("sheet-installs");
}

// -------------------------------------------------------------------- install

async function openInstall() {
  try {
    const help = await invoke("install_help");
    $("install-guidance").textContent = help.guidance;
    $("install-steps").replaceChildren(
      ...help.steps.map((step) => {
        const box = document.createElement("div");
        box.className = "install-step";
        const label = document.createElement("div");
        label.className = "label";
        label.textContent = step.label;
        const code = document.createElement("code");
        code.textContent = step.command;
        const copy = document.createElement("button");
        copy.type = "button";
        copy.className = "copy";
        copy.textContent = help.copy_label;
        copy.addEventListener("click", async () => {
          try {
            await navigator.clipboard.writeText(step.command);
            toast(t("copied"));
          } catch (error) {
            toast(String(error), { error: true });
          }
        });
        box.append(label, code, copy);
        return box;
      }),
    );
    openSheet("sheet-install");
  } catch (error) {
    toast(String(error), { error: true });
  }
}

// ----------------------------------------------------------------------- trust

function openTrust() {
  const trust = state.trust;
  if (!trust) return;
  $("trust-headline").textContent = trust.headline;
  $("trust-reason").textContent = trust.reason;
  $("trust-check").textContent = trust.check;
  $("trust-command").textContent = trust.command;
  $("trust-note").textContent = trust.note;
  $("trust-never").textContent = trust.never;
  $("trust-close").textContent = trust.close;
  openSheet("sheet-trust", "trust-close");
}

// -------------------------------------------------------------------- settings

function selectPage(page) {
  $("nav-launcher").classList.toggle("is-active", page === "launcher");
  $("nav-about").classList.toggle("is-active", page === "about");
  $("page-launcher").hidden = page !== "launcher";
  $("page-about").hidden = page !== "about";
}

function syncSettings() {
  $("set-autostart").checked = state.info.autostart;
  $("set-autoupdate").checked = state.settings.autoupdate;
  $("set-tray").checked = state.settings.tray;
  $("set-tray").disabled = !state.info.tray;
  $("tray-note").hidden = state.info.tray;
}

function openSettings(page = "launcher") {
  selectPage(page);
  syncSettings();
  openSheet("sheet-settings", page === "about" ? "nav-about" : "nav-launcher");
}

// ---------------------------------------------------------------------- update

function setUpdateMark(kind) {
  const mark = $("update-mark");
  const paths = {
    download:
      '<path d="M12 3.5v11"/><path d="m7.5 10 4.5 4.5 4.5-4.5"/><path d="M4.5 17.5v1A2.5 2.5 0 0 0 7 21h10a2.5 2.5 0 0 0 2.5-2.5v-1"/>',
    check: '<circle cx="12" cy="12" r="9"/><path d="m8 12.5 2.6 2.6L16.5 9"/>',
    alert: '<circle cx="12" cy="12" r="9"/><path d="M12 8v5"/><path d="M12 16.2v.1"/>',
  };
  mark.innerHTML = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true">${paths[kind] || paths.download}</svg>`;
}

async function runUpdate({ quiet = false } = {}) {
  $("update").disabled = true;
  $("update-label").textContent = t("update_busy");
  if (!quiet) {
    openSheet("sheet-update");
    setUpdateMark("download");
    $("update-headline").textContent = t("update_checking");
    $("update-detail").textContent = t("update_checking_detail");
    $("update-proof").textContent = "";
    $("update-install").hidden = true;
    $("update-cancel").textContent = t("update_later");
  }
  try {
    const result = await invoke("update_check");
    state.update = result;
    $("update-label").textContent = t("update_ready");
    $("update-dot").hidden = false;
    if (!quiet) {
      if (result.published === result.current) {
        setUpdateMark("check");
        $("update-headline").textContent = t("update_none");
        $("update-detail").textContent = t("update_none_detail");
        $("update-proof").textContent = "";
        $("update-install").hidden = true;
      } else {
        setUpdateMark("download");
        $("update-headline").textContent = fill(t("update_available"), {
          version: result.published,
        });
        $("update-detail").textContent = fill(t("update_running"), {
          current: result.current,
          files: result.files,
        });
        $("update-proof").textContent = fill(t("update_proof"), { key: result.key_id });
        $("update-install").textContent = fill(t("update_install"), {
          version: result.published,
        });
        $("update-install").hidden = false;
      }
    }
  } catch (error) {
    $("update-label").textContent = t("update");
    if (quiet) {
      // An automatic check that cannot reach the release page is not news; the manual
      // button is where the reason is shown.
      $("update-label").textContent = t("update");
    } else {
      setUpdateMark("alert");
      $("update-headline").textContent = t("update_failed");
      $("update-detail").textContent = String(error);
      $("update-proof").textContent = t("update_failed_detail");
      $("update-install").hidden = true;
    }
  } finally {
    $("update").disabled = false;
  }
}

async function runUpdateApply() {
  if (!state.update) return;
  $("update-install").disabled = true;
  $("update-install").hidden = true;
  $("update-headline").textContent = t("update_installing");
  $("update-detail").textContent = t("update_installing_detail");
  $("update-proof").textContent = "";
  try {
    await invoke("update_apply", { stagedDir: state.update.staged_dir });
    setUpdateMark("check");
    $("update-headline").textContent = t("update_done");
    $("update-detail").textContent = "";
    setTimeout(() => void invoke("quit_now"), 1000);
  } catch (error) {
    setUpdateMark("alert");
    $("update-headline").textContent = t("update_broken");
    $("update-detail").textContent = String(error);
    $("update-proof").textContent = t("update_rolledback");
    $("update-install").disabled = false;
    $("update-install").hidden = false;
  }
}

// ------------------------------------------------------------------------ menu

function toggleMenu(force) {
  const open = typeof force === "boolean" ? force : $("menu").hidden;
  // Checking an installation that is not there has no meaning; the item says so by being
  // disabled rather than by opening an installation guide under a diagnostic label.
  $("menu-integrity").disabled = !selectedInstall();
  $("menu").hidden = !open;
  $("menu-toggle").setAttribute("aria-expanded", String(open));
  $("menu-icon").innerHTML = open
    ? '<path d="M4 4l16 16M20 4L4 20" stroke-width="2.4" stroke-linecap="round"/>'
    : '<path d="M4 8h16M4 12h16M4 16h16" stroke-width="2.4" stroke-linecap="butt"/>';
}

function closeMenu() {
  if ($("menu").hidden) return;
  toggleMenu(false);
}

// ------------------------------------------------------------------- wiring

function wire() {
  $("primary").addEventListener("click", () => {
    if (state.busy) return;
    switch (state.copy.action) {
      case "start":
        void runForm("start");
        break;
      case "repair":
        void askForPlan();
        break;
      case "install":
        void openInstall();
        break;
      case "choose":
        openInstalls();
        break;
      default:
        break;
    }
  });

  $("menu-toggle").addEventListener("click", (event) => {
    event.stopPropagation();
    toggleMenu();
  });
  $("menu-settings").addEventListener("click", () => openSettings("launcher"));
  $("menu-shortcut").addEventListener("click", async () => {
    closeMenu();
    try {
      await invoke("create_shortcut");
      toast(t("shortcut_done"));
    } catch (error) {
      toast(String(error), { error: true });
    }
  });
  $("menu-integrity").addEventListener("click", () => {
    closeMenu();
    void openDetails();
  });
  $("menu-cache").addEventListener("click", async () => {
    closeMenu();
    try {
      await invoke("clear_cache");
      toast(t("cache_done"));
    } catch (error) {
      toast(String(error), { error: true });
    }
  });

  document.addEventListener("click", (event) => {
    if (!event.target.closest(".launch")) closeMenu();
  });

  $("update").addEventListener("click", () => void runUpdate());
  $("update-install").addEventListener("click", () => void runUpdateApply());
  $("update-cancel").addEventListener("click", closeSheet);

  $("settings").addEventListener("click", () => openSettings("launcher"));
  $("minimize").addEventListener("click", () => void invoke("window_action", { action: "minimize" }));
  $("close").addEventListener("click", async () => {
    if (!state.info.tray || !state.settings.tray) {
      await invoke("window_action", { action: "quit" });
      return;
    }
    if (state.settings.closeNotice) {
      await invoke("window_action", { action: "hide" });
      return;
    }
    openSheet("sheet-close", "close-keep");
  });
  $("close-keep").addEventListener("click", async () => {
    if ($("close-remember").checked) {
      state.settings.closeNotice = true;
      saveSettings();
    }
    closeSheet();
    await invoke("window_action", { action: "hide" });
  });
  $("close-quit").addEventListener("click", async () => {
    if ($("close-remember").checked) {
      state.settings.closeNotice = true;
      saveSettings();
    }
    await invoke("window_action", { action: "quit" });
  });

  $("all-installs").addEventListener("click", openInstalls);
  $("detail-link").addEventListener("click", () => void openDetails());
  $("details-close").addEventListener("click", closeSheet);
  $("details-copy").addEventListener("click", () => void copyDiagnostics());
  $("details-undo").addEventListener("click", () => void runUndo());
  $("details-repair").addEventListener("click", () => {
    closeSheet();
    void askForPlan();
  });

  $("plan-cancel").addEventListener("click", closeSheet);
  $("plan-confirm").addEventListener("click", () => void runRepair());

  $("nav-launcher").addEventListener("click", () => selectPage("launcher"));
  $("nav-about").addEventListener("click", () => selectPage("about"));
  $("about-update").addEventListener("click", () => void runUpdate());
  $("about-trust").addEventListener("click", openTrust);
  $("about-copy").addEventListener("click", () => void copyDiagnostics());
  $("about-repo").addEventListener("click", () => void invoke("open_url", { url: REPO }));
  $("about-log").addEventListener("click", () => void invoke("open_log"));

  $("set-autoupdate").addEventListener("change", (event) => {
    state.settings.autoupdate = event.target.checked;
    saveSettings();
  });
  $("set-tray").addEventListener("change", (event) => {
    state.settings.tray = event.target.checked;
    saveSettings();
  });
  $("set-autostart").addEventListener("change", async (event) => {
    const wanted = event.target.checked;
    event.target.disabled = true;
    try {
      const now = await invoke("set_autostart", { enabled: wanted });
      state.info.autostart = now;
      event.target.checked = now;
      if (now !== wanted) toast(t("setting_failed"), { error: true });
    } catch (error) {
      event.target.checked = !wanted;
      toast(String(error), { error: true });
    } finally {
      event.target.disabled = false;
    }
  });

  $("after-start").addEventListener("change", (event) => {
    state.settings.afterStart = event.target.checked;
    saveSettings();
  });

  $("open-link").addEventListener("click", () => {
    if (state.url) void invoke("open_url", { url: state.url });
  });

  $("overlay").addEventListener("click", (event) => {
    if (event.target === $("overlay")) closeSheet();
  });
  for (const button of document.querySelectorAll("[data-close]")) {
    button.addEventListener("click", closeSheet);
  }

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!$("menu").hidden) {
      closeMenu();
      return;
    }
    if (!$("overlay").hidden) closeSheet();
  });

  window.__TAURI__?.event?.listen?.("update-requested", () => void runUpdate());
}

// ------------------------------------------------------------------------ boot

async function refresh() {
  let rows = await invoke("list");
  if (params.has("empty")) rows = [];
  state.installs = rows;
  state.byKind = { desktop: [], web: [], tui: [] };
  for (const row of rows) {
    (state.byKind[row.kind] ||= []).push(row);
  }
  state.kindState = {};
  for (const kind of ORDER) {
    // Installed but not yet checked is not the same as healthy. The badge stays grey until
    // this window has actually run the check for that distribution.
    state.kindState[kind] = state.byKind[kind].length ? "unknown" : "absent";
  }
  // `?kind=web` starts on a chosen rail slot. It exists so each distribution's screen can
  // be photographed without a mouse; the rail does the same thing with a click.
  const wanted = params.get("kind");
  const first =
    (wanted && state.byKind[wanted] ? wanted : null) ||
    ORDER.find((kind) => state.byKind[kind].length) ||
    "desktop";
  state.kind = first;
  renderTabs();
  await select(first);
}

async function boot() {
  loadSettings();
  try {
    const [chrome, info, trust, labels] = await Promise.all([
      invoke("chrome"),
      invoke("shell_info"),
      invoke("trust_help"),
      invoke("form_action", { installId: "", action: "labels" }).catch(() => ({})),
    ]);
    state.chrome = chrome;
    state.info = info;
    state.trust = trust;
    state.labels = labels || {};
  } catch (error) {
    $("headline").textContent = String(error);
    $("reason").textContent = "";
    return;
  }

  applyChrome();
  wire();
  $("after-start").checked = state.settings.afterStart;

  try {
    await refresh();
  } catch (error) {
    // A window without its kernel is the one state where the copy crate cannot describe
    // the machine for it; the sentence comes from the shell, and the raw reason stays
    // visible because it is what a bug report needs.
    $("headline").textContent = t("kernel_missing");
    $("reason").textContent = `${String(error)} — ${t("kernel_hint")}`;
    $("headline").classList.add("long");
    return;
  }

  // WebView2 hands focus to the first focusable element when the window activates. A yellow
  // ring on a control nobody chose reads as a selection, so the board starts without one;
  // the first Tab puts it back.
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();

  if (state.settings.autoupdate) {
    setTimeout(() => void runUpdate({ quiet: true }), 1500);
  }

  const mode = params.get("autorepair");
  if (params.has("settings")) openSettings(params.has("about") ? "about" : "launcher");
  if (params.has("menu")) toggleMenu(true);
  if (params.has("details")) void openDetails();
  if (params.has("update")) void runUpdate();
  if (params.has("trust")) openTrust();
  if (params.has("guide")) void openInstall();
  if (params.has("close")) openSheet("sheet-close", "close-keep");
  if (mode !== null) {
    await askForPlan();
    if (mode === "slow" || mode === "hold") return;
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await runRepair();
  }
}

boot();
