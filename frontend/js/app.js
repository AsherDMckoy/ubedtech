// Progressive enhancement only — every page works with JavaScript off
// (FRONTEND.md §5: fragment interactions are enhancement over working HTML
// forms). Alpine's CSP build is the sanctioned interactivity layer; screen
// components register themselves with Alpine.data() here as later sessions
// add them.
import Alpine from "@alpinejs/csp";

// Honest in-progress state: disable a form's buttons and swap any button
// carrying data-busy-label to that label ("Checking…", "Submitting…"). The
// server-side idempotency keys remain the real duplicate guarantee.
function setBusy(form) {
  form.setAttribute("aria-busy", "true");
  for (const button of form.querySelectorAll("button")) {
    button.disabled = true;
    if (button.dataset.busyLabel) {
      button.dataset.restoreLabel = button.textContent;
      button.textContent = button.dataset.busyLabel;
    }
  }
}

// Reverse setBusy — used when a fragment request fails and after a bfcache
// restore, so a returned-to page is never stuck in its busy state.
function restoreForm(form) {
  form.removeAttribute("aria-busy");
  for (const button of form.querySelectorAll("button[disabled]")) {
    button.disabled = false;
    if (button.dataset.restoreLabel) {
      button.textContent = button.dataset.restoreLabel;
      delete button.dataset.restoreLabel;
    }
  }
}

// Announce a mutation outcome to assistive tech via the page's live region.
function announce(message) {
  const live = document.querySelector("[data-live]");
  if (live) live.textContent = message;
}

// Submit-once + busy state for ordinary forms. Fragment forms suppress this
// in the capture phase below (a drop waits for confirmation first).
addEventListener("submit", (event) => {
  const form = event.target;
  if (form.getAttribute("aria-busy") === "true") {
    event.preventDefault();
    return;
  }
  setBusy(form);
});

// Restore forms when a page returns from the back/forward cache.
addEventListener("pageshow", (event) => {
  if (!event.persisted) {
    return;
  }
  for (const form of document.querySelectorAll("form[aria-busy]")) {
    restoreForm(form);
  }
});

// Native <dialog> wiring (animated surface #2): a button with
// data-dialog-open="id" opens that dialog modally (focus trap and Escape
// come from the platform); anything with data-dialog-close closes its
// enclosing dialog. No JS → the dialog's fallback link/form still works.
addEventListener("click", (event) => {
  const opener = event.target.closest("[data-dialog-open]");
  if (opener) {
    document.getElementById(opener.dataset.dialogOpen)?.showModal();
    return;
  }
  if (event.target.closest("[data-dialog-close]")) {
    event.target.closest("dialog")?.close();
    return;
  }
  // Print buttons on the unofficial documents (CSP forbids inline handlers).
  if (event.target.closest("[data-print]")) {
    window.print();
  }
});

// Confirm-gated full-page forms (publish grades): the first submit opens
// the named dialog instead; [data-dialog-confirm] resubmits for real
// (form.submit() skips the submit handlers). With JS off the form POSTs
// directly — the dialog is enhancement, the POST is the action.
let pendingConfirm = null;

addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!form.matches("[data-confirm-dialog]")) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    pendingConfirm = form;
    document.getElementById(form.dataset.confirmDialog)?.showModal();
  },
  true,
);

addEventListener("click", (event) => {
  if (!event.target.closest("[data-dialog-confirm]")) return;
  event.target.closest("dialog")?.close();
  if (pendingConfirm) {
    const form = pendingConfirm;
    pendingConfirm = null;
    setBusy(form);
    form.submit();
  }
});

// Honest status polling (documents): rows carrying data-poll re-fetch
// their server-rendered fragment and swap it in — the status shown is
// always the real backend row, never a predicted one. Terminal rows render
// without data-poll, so polling stops by itself. Pure enhancement: with JS
// off, a reload shows the same truth.
if (document.querySelector("[data-poll]")) {
  setInterval(async () => {
    for (const row of document.querySelectorAll("[data-poll]")) {
      try {
        const response = await fetch(row.dataset.poll, {
          credentials: "same-origin",
        });
        if (!response.ok) continue;
        const html = (await response.text()).trim();
        if (!html.startsWith("<tr")) continue;
        const changed = !html.includes(`data-status="${row.dataset.status}"`);
        row.outerHTML = html;
        if (changed) {
          announce("A document request changed status");
        }
      } catch {
        // Network hiccup: the next tick retries; the row keeps its state.
      }
    }
  }, 4000);
}

// ---- Registration screen ------------------------------------------------
// Read path: filter the already-loaded rows in place, no round trip
// (FRONTEND.md §4). With JS off the search form GETs and the server filters.
addEventListener("input", (event) => {
  const form = event.target.closest("[data-search-form]");
  if (!form) return;
  const needle = event.target.value.trim().toLowerCase();
  const rows = document.querySelectorAll("tr[data-search]");
  let shown = 0;
  for (const row of rows) {
    const hit = !needle || row.dataset.search.includes(needle);
    row.hidden = !hit;
    if (hit) shown += 1;
  }
  const empty = document.querySelector("[data-search-empty]");
  if (empty) empty.hidden = shown !== 0;
  const count = document.querySelector("[data-search-count]");
  if (count) {
    count.textContent = needle
      ? `Showing ${shown} of ${rows.length}`
      : `Showing all ${rows.length} sections`;
  }
});

// Read path: sortable scanning tables (registrar overview). A th button
// carrying data-sort reorders the already-loaded rows by that column,
// numeric-aware; aria-sort on the th announces the direction. With JS off
// the server's default order stands — sorting is presentation, not data.
addEventListener("click", (event) => {
  const button = event.target.closest("th button[data-sort]");
  if (!button) return;
  const th = button.closest("th");
  const table = th.closest("table");
  const index = [...th.parentNode.children].indexOf(th);
  const ascending = th.getAttribute("aria-sort") !== "ascending";
  for (const header of table.querySelectorAll("th[aria-sort]")) {
    header.removeAttribute("aria-sort");
  }
  th.setAttribute("aria-sort", ascending ? "ascending" : "descending");
  const body = table.tBodies[0];
  const rows = [...body.querySelectorAll("tr[data-search]")];
  const key = (row) => {
    const text = row.children[index].textContent.trim();
    const number = Number.parseFloat(text.replace("%", ""));
    return Number.isNaN(number) ? text.toLowerCase() : number;
  };
  rows.sort((a, b) => (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));
  if (!ascending) rows.reverse();
  body.prepend(...rows);
});

// Write path: register/drop go to the server and reflect the committed
// outcome via a single-row swap — never an optimistic success (FRONTEND.md
// §3). A drop is destructive, so it confirms first. Capture phase, so a
// confirm-gated drop can suppress the busy handler until it is confirmed.
let pendingDrop = null;

addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!form.matches("[data-fragment]")) return;
    if (form.hasAttribute("data-confirm")) {
      event.preventDefault();
      event.stopImmediatePropagation();
      pendingDrop = form;
      const dialog = document.getElementById("drop-dialog");
      const name = dialog?.querySelector("[data-drop-name]");
      if (name) {
        name.textContent = `You'll lose your seat in ${form.dataset.confirm}. You can re-register while add/drop is open, if seats remain.`;
      }
      dialog?.showModal();
      return;
    }
    // Register: let the bubble-phase submit-once handler set "Checking…",
    // then swap in the server's answer.
    event.preventDefault();
    submitRow(form);
  },
  true,
);

addEventListener("click", (event) => {
  if (!event.target.closest("[data-drop-confirm]")) return;
  event.target.closest("dialog")?.close();
  if (pendingDrop) {
    const form = pendingDrop;
    pendingDrop = null;
    setBusy(form);
    submitRow(form);
  }
});

async function submitRow(form) {
  const row = form.closest("tr");
  try {
    const response = await fetch(form.action, {
      method: "POST",
      body: new FormData(form),
      headers: { "X-Fragment": "row" },
      credentials: "same-origin",
    });
    const html = (await response.text()).trim();
    if (row && html.startsWith("<tr")) {
      row.outerHTML = html;
      announce("Your registration was updated");
    } else {
      restoreForm(form);
    }
  } catch {
    restoreForm(form);
  }
}

window.Alpine = Alpine;
Alpine.start();
