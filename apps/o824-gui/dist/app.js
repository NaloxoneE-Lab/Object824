// The window's whole job: ask the kernel, draw the answer.
//
// Every sentence on screen comes from the Rust side (`o824-present`), which is where the
// copy is tested for jargon and for having a next step. Nothing here decides what a
// finding means, and nothing here talks to the kernel directly — it cannot, deliberately:
// §7.5 gives the process handle to whoever spawns, and that is the Rust side.

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
const TRUST = "Unofficial tool · unsigned · github.com/NaloxoneE-Lab/Object824";

function drawScreen(screen) {
  elements.trust.textContent = TRUST;
  elements.headline.textContent = screen.headline;
  elements.reason.textContent = screen.reason;

  if (screen.next_step) {
    elements.primary.textContent = screen.next_step;
    elements.primary.hidden = false;
  }

  elements.installations.hidden = screen.installations.length === 0;
  elements.installations.replaceChildren(
    ...screen.installations.map((install) => {
      const row = document.createElement("button");
      row.className = "row";
      const name = document.createElement("span");
      name.textContent = `${install.name} ${install.version}`;
      const detail = document.createElement("span");
      detail.className = "detail";
      detail.textContent =
        install.findings === 0
          ? "no problems found"
          : `${install.findings} problem(s), ${install.repairable} fixable automatically`;
      row.append(name, detail);
      // A click asks for the detail; the detail is a sentence per finding.
      row.addEventListener("click", () => showFindings(install.id));
      return row;
    }),
  );

  elements.copy.hidden = screen.installations.length === 0;
  elements.status.textContent = "";
}

async function showFindings(installId) {
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
  try {
    const screen = await invoke("screen");
    drawScreen(screen);
  } catch (error) {
    showFailure(String(error));
  }
}

elements.primary?.addEventListener("click", () => {
  // The first slice shows the sentence; the repair itself is the next slice.
  elements.status.textContent = "this build shows the diagnosis only";
});

elements.copy?.addEventListener("click", async () => {
  try {
    const screen = await invoke("screen");
    await navigator.clipboard.writeText(JSON.stringify(screen, null, 2));
    elements.status.textContent = "copied";
  } catch (error) {
    elements.status.textContent = String(error);
  }
});

start();
