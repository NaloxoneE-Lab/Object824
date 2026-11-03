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
  if (copy.next_step) {
    elements.primary.textContent = copy.next_step;
    elements.primary.hidden = false;
  } else {
    elements.primary.hidden = true;
  }
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
  try {
    setHeadline(
      await invoke("summarize", {
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
    setHeadline(await invoke("summarize", { installs: [result] }));
    await showFindings(install.id, { quiet: true });
  } catch (error) {
    if (detail()) detail().textContent = "could not be checked";
    showFailure(String(error));
  }
}

elements.primary?.addEventListener("click", () => {
  // The first slice shows the sentence; the repair itself is the next slice.
  elements.status.textContent = "this build shows the diagnosis only";
});

elements.copy?.addEventListener("click", async () => {
  try {
    const text = [elements.headline.textContent, elements.reason.textContent].join("\n");
    await navigator.clipboard.writeText(text);
    elements.status.textContent = "copied";
  } catch (error) {
    elements.status.textContent = String(error);
  }
});

start();
