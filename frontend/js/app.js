// Progressive enhancement only — every page works with JavaScript off
// (FRONTEND.md §5: fragment interactions are enhancement over working HTML
// forms). Alpine's CSP build is the sanctioned interactivity layer; screen
// components register themselves with Alpine.data() here as later sessions
// add them.
import Alpine from "@alpinejs/csp";

// Submit-once + honest in-progress state: after a form is submitted, block
// a second submission, disable its buttons, and swap any button carrying
// data-busy-label to that label ("Checking…", "Submitting…"). The
// server-side idempotency keys remain the real duplicate guarantee.
addEventListener("submit", (event) => {
  const form = event.target;
  if (form.getAttribute("aria-busy") === "true") {
    event.preventDefault();
    return;
  }
  form.setAttribute("aria-busy", "true");
  for (const button of form.querySelectorAll("button")) {
    button.disabled = true;
    if (button.dataset.busyLabel) {
      button.dataset.restoreLabel = button.textContent;
      button.textContent = button.dataset.busyLabel;
    }
  }
});

// Restore forms when a page returns from the back/forward cache.
addEventListener("pageshow", (event) => {
  if (!event.persisted) {
    return;
  }
  for (const form of document.querySelectorAll("form[aria-busy]")) {
    form.removeAttribute("aria-busy");
    for (const button of form.querySelectorAll("button[disabled]")) {
      button.disabled = false;
      if (button.dataset.restoreLabel) {
        button.textContent = button.dataset.restoreLabel;
        delete button.dataset.restoreLabel;
      }
    }
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
  }
});

window.Alpine = Alpine;
Alpine.start();
