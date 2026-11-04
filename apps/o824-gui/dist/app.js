// The window's whole job: ask the kernel, draw the answer.
//
// Every sentence on screen comes from the Rust side (`o824-present`), which is where the
// copy is tested for jargon and for having a next step. Nothing here decides what a
// finding means, and nothing here talks to the kernel directly — it cannot, deliberately:
// §7.5 gives the process handle to whoever spawns, and that is the Rust side.
//
// The one thing this file does decide is *when*: the list is drawn as soon as discovery
// answers (about a tenth of a second), and each row is diagnosed separately because a
// diagnosis walks every plugin folder and takes seconds. Waiting for three of them behind a
// blank page is what made the window look stuck.

const invoke = window.__TAURI__.core.invoke;

const elements = {
  trust: document.getElementById("trust"),
  headline: document.getElementById("headline"),
  reason: document.getElementById("reason"),
  primary: document.getElementById("primary"),
  installations: document.getElementById("installations"),
  findings: document.getElementById("findings"),
  failure: document.getElementById("failure"),
  panel: document.getElementById("confirm-panel"),
  backup: document.getElementById("backup"),
  steps: document.getElementById("steps"),
  stepsHeading: document.getElementById("steps-heading"),
  installPanel: document.getElementById("install-panel"),
  installGuidance: document.getElementById("install-guidance"),
  installSteps: document.getElementById("install-steps"),
  confirmStart: document.getElementById("confirm-start"),
  confirmCancel: document.getElementById("confirm-cancel"),
  formButtons: document.getElementById("form-buttons"),
  formStop: document.getElementById("form-stop"),
  formUndo: document.getElementById("form-undo"),
  formUrl: document.getElementById("form-url"),
  copy: document.getElementById("copy"),
  status: document.getElementById("status"),
};

// §20.6: the trust statement is fixed and always visible, and it says "unsigned" out loud.
const TRUST = "Unofficial tool · unsigned · github.com/Naloxone-E-Lab/Object824";

// What each row says while its diagnosis is still running.
const CHECKING = "checking…";

function setHeadline(copy) {
  elements.headline.textContent = copy.headline;
  elements.reason.textContent = copy.reason;
  setPrimary(copy);
}

/// The geometry, so a layout problem is measured instead of guessed from a screenshot.
function layoutMeasurements() {
  const main = document.querySelector("main");
  const row = document.querySelector(".row");
  const detail = document.querySelector('[data-role="detail"]');
  const rect = (node) => {
    if (!node) return null;
    const r = node.getBoundingClientRect();
    return { left: Math.round(r.left), right: Math.round(r.right), width: Math.round(r.width) };
  };
  return {
    viewport: { width: window.innerWidth, height: window.innerHeight },
    document: {
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    },
    main: rect(main),
    row: rect(row),
    detail: rect(detail),
  };
}

/// §20: guidance, not an install performed behind the user's back.
async function showInstallHelp() {
  try {
    const help = await invoke("install_help");
    elements.installGuidance.textContent = help.guidance;
    elements.installSteps.replaceChildren(
      ...help.steps.map((step) => {
        const item = document.createElement("li");
        const name = document.createElement("span");
        name.textContent = `${step.label}: `;
        const code = document.createElement("code");
        code.textContent = step.command;
        item.append(name, code);
        return item;
      }),
    );
    elements.installPanel.hidden = false;
  } catch (error) {
    showFailure(String(error));
  }
}

/// Offers the counterpart action: stopping what is running, and opening what has an address.
async function showFormButtons() {
  try {
    const labels = await invoke("form_action", { installId: "", action: "labels" });
    elements.formStop.textContent = labels.stop_label;
    elements.formUrl.textContent = labels.open_label;
  } catch (error) {
    /* labels are cosmetic; the actions themselves report their own errors */
  }
  elements.formButtons.hidden = false;
  elements.formStop.hidden = false;
}

function drawRows(rows) {
  elements.installations.hidden = rows.length === 0;
  elements.installations.replaceChildren(
    ...rows.map((install) => {
      const row = document.createElement("button");
      row.className = "row";
      row.dataset.id = install.id;

      const name = document.createElement("span");
      name.textContent = `${install.name} ${install.version}`;

      const detail = document.createElement("span");
      detail.className = "detail";
      detail.dataset.role = "detail";
      detail.textContent = CHECKING;

      row.append(name, detail);
      row.addEventListener("click", () => diagnoseRow(install));
      return row;
    }),
  );
}

/// The detail line for a diagnosed row.
function detailFor(row) {
  if (row.findings === 0) return "no problems found";
  const fixable = row.repairable > 0 ? `, ${row.repairable} fixable automatically` : "";
  return `${row.findings} problem(s)${fixable}`;
}

async function showFindings(installId, options = {}) {
  if (!options.quiet) elements.status.textContent = "reading the details…";
  try {
    const findings = await invoke("diagnose", { installId });
    elements.findings.hidden = findings.length === 0;
    elements.findings.replaceChildren(
      ...findings.map((finding) => {
        const card = document.createElement("article");
        card.className = "card";
        const title = document.createElement("h2");
        title.textContent = finding.headline;
        const reason = document.createElement("p");
        reason.textContent = finding.reason;
        const next = document.createElement("p");
        next.className = "next";
        next.textContent = finding.next_step;
        card.append(title, reason, next);
        return card;
      }),
    );
    elements.status.textContent = "";
  } catch (error) {
    showFailure(String(error));
  }
}

function showFailure(message) {
  // §20.2 again: a failure with no next step is not finished. The command line still
  // works, and that is the step.
  elements.failure.hidden = false;
  elements.failure.textContent = `${message} — you can still run "o824 doctor" from a terminal.`;
  elements.status.textContent = "the kernel did not answer";
}

async function start() {
  elements.trust.textContent = TRUST;

  // First what the window says while it works, then the list, then the diagnoses.
  try {
    setHeadline(await invoke("checking", { localeTag: null }));
  } catch (error) {
    showFailure(String(error));
    return;
  }

  let rows;
  try {
    rows = await invoke("list");
  } catch (error) {
    showFailure(String(error));
    return;
  }

  drawRows(rows);
  elements.copy.hidden = rows.length === 0;

  if (rows.length === 1) {
    lastDiagnosed = rows[0];
    // One installation: diagnosing it is the first screen's job, and the user sees the
    // result without having to ask. §20.1's third path.
    await diagnoseRow(rows[0]);
    return;
  }

  // Several installations: diagnosing all of them would take half a minute for answers
  // nobody asked for. The design says "pick one to work on", so the rows say what they are
  // and a click diagnoses the one that was chosen.
  rows.forEach((install) => {
    const detail = elements.installations.querySelector(
      `[data-id="${install.id}"] [data-role="detail"]`,
    );
    if (detail) detail.textContent = "click to check";
  });
  chosen = null;
  try {
    setHeadline(
      await invoke("summarize", {
        layout: layoutMeasurements(),
        installs: rows.map((row) => ({
          kind: row.kind,
          version: row.version,
          findings: 0,
          repairable: 0,
        })),
      }),
    );
  } catch (error) {
    showFailure(String(error));
  }
}

/// Diagnoses one row, in place, and folds the answer into the headline.
async function diagnoseRow(install) {
  const detail = () =>
    elements.installations.querySelector(`[data-id="${install.id}"] [data-role="detail"]`);
  if (detail()) detail().textContent = CHECKING;
  elements.status.textContent = "checking this one…";
  try {
    const findings = await invoke("diagnose", { installId: install.id });
    const repairable = findings.filter((finding) => finding.repairable).length;
    const result = {
      kind: install.kind,
      version: install.version,
      findings: findings.length,
      repairable,
    };
    if (detail()) detail().textContent = detailFor(result);
    elements.status.textContent = "";
    lastDiagnosed = install;
    showFormButtons();
    showFormButtons();
    setHeadline(
      await invoke("summarize", { layout: layoutMeasurements(), installs: [result] }),
    );
    await showFindings(install.id, { quiet: true });
  } catch (error) {
    if (detail()) detail().textContent = "could not be checked";
    showFailure(String(error));
  }
}

/// The install the primary button is about, once something is chosen.
let chosen = null;

/// The installation the headline describes, so the primary button knows its subject.
let lastDiagnosed = null;

/// Asks for a plan and shows it. Nothing has changed at this point.
async function askForPlan(install) {
  chosen = install;
  elements.status.textContent = "asking what a repair would do…";
  try {
    const plan = await invoke("repair_plan", { installId: install.id });
    elements.backup.textContent = plan.backup;
    elements.stepsHeading.textContent = plan.heading;
    elements.steps.replaceChildren(
      ...plan.steps.map((text) => {
        const item = document.createElement("li");
        item.textContent = text;
        return item;
      }),
    );
    elements.confirmStart.textContent = plan.confirm;
    elements.confirmCancel.textContent = plan.cancel;
    elements.panel.hidden = false;
    elements.status.textContent = "";
    // §20.7: the default focus is the way out, not the way in.
    elements.confirmCancel.focus();
  } catch (error) {
    elements.status.textContent = "";
    showFailure(String(error));
  }
}

async function runRepair() {
  if (!chosen) return;
  elements.panel.hidden = true;
  elements.primary.disabled = true;
  elements.status.textContent = "repairing…";
  try {
    const result = await invoke("repair_apply", { installId: chosen.id });
    elements.headline.textContent = result.headline;
    elements.reason.textContent = result.reason;
    elements.primary.hidden = !result.next_step;
    setPrimary({ next_step: result.next_step, action: result.findings === 0 ? "start" : "repair" });
    elements.status.textContent = result.applied
      ? result.findings === 0
        ? "done"
        : `${result.findings} problem(s) left`
      : "the repair did not finish";
    // Both the row and the detail list are stale now.
    await refreshRow(chosen);
    await showFindings(chosen.id, { quiet: true });
    // §20.3: what was just changed can be put back, and the button says so.
    if (result.applied) {
      try {
        const labels = await invoke("form_action", { installId: "", action: "labels" });
        elements.formUndo.textContent = labels.undo_label;
      } catch (error) {
        elements.formUndo.textContent = "Undo";
      }
      elements.formUndo.hidden = false;
    }
  } catch (error) {
    showFailure(String(error));
  } finally {
    elements.primary.disabled = false;
  }
}

/// Re-diagnoses one row after a change.
async function refreshRow(install) {
  try {
    const findings = await invoke("diagnose", { installId: install.id });
    const repairable = findings.filter((finding) => finding.repairable).length;
    const detail = elements.installations.querySelector(
      `[data-id="${install.id}"] [data-role="detail"]`,
    );
    if (detail) {
      detail.textContent = detailFor({ findings: findings.length, repairable });
    }
  } catch (error) {
    /* the headline already says what happened */
  }
}

/// What the main button is currently for, from the copy crate's own decision.
let primaryAction = "";

function setPrimary(copy) {
  if (!copy.next_step) {
    elements.primary.hidden = true;
    primaryAction = "";
    return;
  }
  elements.primary.textContent = copy.next_step;
  elements.primary.hidden = false;
  primaryAction = copy.action || "";
}

elements.primary?.addEventListener("click", () => {
  const target = chosen || lastDiagnosed;
  switch (primaryAction) {
    case "start":
      if (target) void runFormAction("start");
      break;
    case "repair":
      if (target) void askForPlan(target);
      break;
    case "install":
      void showInstallHelp();
      break;
    case "choose":
      elements.installations.querySelector(".row")?.focus();
      break;
    default:
      break;
  }
});

elements.confirmCancel?.addEventListener("click", () => {
  elements.panel.hidden = true;
  elements.status.textContent = "";
});

elements.confirmStart?.addEventListener("click", () => void runRepair());

/// Starts or stops the chosen form, then says what state it is in.
async function runFormAction(action) {
  const install = chosen || lastDiagnosed;
  if (!install) return;
  elements.status.textContent = action === "start" ? "starting…" : "stopping…";
  elements.formStop.disabled = true;
  try {
    const result = await invoke("form_action", { installId: install.id, action });
    elements.formStop.textContent = result.stop_label;
    elements.formUrl.textContent = result.open_label;
    elements.formUrl.href = result.url || "#";
    elements.formUrl.hidden = !result.url;
    elements.status.textContent = result.message;
    chosen = install;
  } catch (error) {
    elements.status.textContent = "";
    showFailure(String(error));
  } finally {
    elements.formStop.disabled = false;
  }
}

elements.formStop?.addEventListener("click", () => void runFormAction("stop"));

/// Undoes the repair that was just applied, and says what the machine looks like now.
async function runUndo() {
  const install = chosen || lastDiagnosed;
  if (!install) return;
  elements.status.textContent = "undoing…";
  elements.formUndo.disabled = true;
  try {
    const result = await invoke("undo_repair", { installId: install.id });
    elements.headline.textContent = result.headline;
    elements.reason.textContent = result.reason;
    elements.status.textContent = result.findings === 0 ? "done" : `${result.findings} problem(s)`;
    await refreshRow(install);
    await showFindings(install.id, { quiet: true });
  } catch (error) {
    showFailure(String(error));
  } finally {
    elements.formUndo.disabled = false;
  }
}

elements.formUndo?.addEventListener("click", () => void runUndo());

elements.copy?.addEventListener("click", async () => {
  try {
    const text = [elements.headline.textContent, elements.reason.textContent].join("\n");
    await navigator.clipboard.writeText(text);
    elements.status.textContent = "copied";
  } catch (error) {
    elements.status.textContent = String(error);
  }
});

const AUTO = new URLSearchParams(location.search).has("autorepair");

start().then(async () => {
  if (!AUTO) return;
  const target = lastDiagnosed || (await invoke("list"))[0];
  if (!target) return;
  await askForPlan(target);
  // `hold` stops at the confirmation panel and `slow` pauses there: both exist so the panel
  // can be photographed. The real pace is a second and a half.
  const mode = new URLSearchParams(location.search).get("autorepair");
  if (mode === "hold") return;
  await new Promise((resolve) => setTimeout(resolve, mode === "slow" ? 8000 : 1500));
  await runRepair();
});
