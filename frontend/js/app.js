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
    // Links double as JS-off fallbacks (sign out -> the confirmation
    // page); with the dialog available, stay on this page.
    event.preventDefault();
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
    const dialog = document.getElementById(form.dataset.confirmDialog);
    const name = dialog?.querySelector("[data-drop-name]");
    if (name && form.dataset.confirm) {
      name.textContent = `You'll lose your seat in ${form.dataset.confirm}. You can re-register while add/drop is open, if seats remain.`;
    }
    dialog?.showModal();
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

// Dashboard smart calendar: hovering OR focusing an event row jumps the
// grid to that event's month, lights its date span, and tints the month
// name; leaving/blurring returns to the current month with today marked.
// Every month is server-rendered — this only toggles visibility (an
// instant content change, not an animated surface).
function calJump(li, on) {
  const cal = document.querySelector("[data-minical]");
  if (!li || !cal) return;
  const month = on ? li.dataset.from.slice(0, 7) : null;
  let index = 0;
  for (const table of cal.querySelectorAll("[data-cal-month]")) {
    table.hidden = month ? table.dataset.calMonth !== month : index !== 0;
    index += 1;
  }
  cal.classList.toggle("is-jumped", !!month);
  for (const day of cal.querySelectorAll("[data-date]")) {
    const hit =
      !!month &&
      li.dataset.from <= day.dataset.date &&
      day.dataset.date <= li.dataset.to;
    day.classList.toggle("cal-hot", hit);
  }
}

addEventListener("mouseover", (event) => {
  calJump(event.target.closest("[data-event]"), true);
});
addEventListener("mouseout", (event) => {
  calJump(event.target.closest("[data-event]"), false);
});
addEventListener("focusin", (event) => {
  calJump(event.target.closest("[data-event]"), true);
});
addEventListener("focusout", (event) => {
  calJump(event.target.closest("[data-event]"), false);
});

// Shell-persistent navigation (slice E): a nav click fetches the target
// page FRESH from the server and swaps only the main region (plus the
// nav's active marks), so the shell isn't re-parsed on every hop. This
// is perceived speed WITHOUT client-side caching: no page data is ever
// stored (docs/SECURITY.md — grades/schedules must not persist on a
// shared machine), and the no-store response headers are untouched.
// Real URLs and working back/forward via the History API; JS off, every
// link is a plain href.
async function swapMain(href, push) {
  const main = document.getElementById("main");
  if (!main) {
    window.location.assign(href);
    return;
  }
  main.setAttribute("aria-busy", "true");
  try {
    const response = await fetch(href, { credentials: "same-origin" });
    const text = await response.text();
    const doc = new DOMParser().parseFromString(text, "text/html");
    const freshMain = doc.getElementById("main");
    // Anything unexpected (signed out -> login page markup, error page,
    // non-shell page): honest full navigation instead of a wrong swap.
    if (!response.ok || !freshMain || !doc.querySelector(".rail")) {
      window.location.assign(href);
      return;
    }
    main.replaceWith(freshMain);
    for (const selector of [".rail", ".sheet-panel"]) {
      const current = document.querySelector(selector);
      const fresh = doc.querySelector(selector);
      if (current && fresh) current.innerHTML = fresh.innerHTML;
    }
    document.title = doc.title;
    if (push) history.pushState({ swap: true }, "", href);
    scrollTo(0, 0);
    freshMain.tabIndex = -1;
    freshMain.focus({ preventScroll: true });
    announce(doc.title);
  } catch {
    window.location.assign(href);
  }
}

addEventListener("click", (event) => {
  const link = event.target.closest(".rail a.navlink, .sheet-panel a.navlink");
  if (!link || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
    return;
  }
  event.preventDefault();
  history.replaceState({ swap: true }, "", location.href);
  link.closest("details.nav-sheet")?.removeAttribute("open");
  swapMain(link.href, true);
});

addEventListener("popstate", (event) => {
  if (event.state?.swap) swapMain(location.href, false);
});

// Schedule month navigation (slice F): month prev/next and day cells are
// real links; the enhancement swaps just the calendar region and keeps
// the URL real via replaceState (a month flip is a view change within
// the page, not a new history entry).
addEventListener("click", async (event) => {
  const link = event.target.closest("a[data-cal-nav]");
  if (!link || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
    return;
  }
  const region = document.getElementById("month-cal");
  if (!region) return;
  event.preventDefault();
  region.setAttribute("aria-busy", "true");
  try {
    const response = await fetch(link.href, { credentials: "same-origin" });
    const text = await response.text();
    const doc = new DOMParser().parseFromString(text, "text/html");
    const fresh = doc.getElementById("month-cal");
    if (!response.ok || !fresh) {
      window.location.assign(link.href);
      return;
    }
    region.replaceWith(fresh);
    history.replaceState(history.state, "", link.href);
    // Move focus for screen readers without yanking the viewport — a day
    // click is a view change within the page, not a navigation.
    fresh.querySelector("#day-detail")?.focus({ preventScroll: true });
  } catch {
    window.location.assign(link.href);
  }
});

// Course-detail dialog: an opener carries data-cd-* attributes with the
// server-rendered facts; clicking copies each into its [data-cd-slot]
// and opens the one shared dialog. Presentation only — the same facts
// live in the row and its expansion, so nothing depends on this.
addEventListener("click", (event) => {
  const opener = event.target.closest("[data-course-open]");
  if (!opener) return;
  const dialog = document.getElementById("course-dialog");
  if (!dialog) return;
  for (const slot of dialog.querySelectorAll("[data-cd-slot]")) {
    const name = slot.dataset.cdSlot;
    slot.textContent = opener.dataset["cd" + name[0].toUpperCase() + name.slice(1)] || "";
  }
  dialog.showModal();
});

// Schedule class-date highlight: weekly rows carry data-dates (that
// class's remaining meeting dates); hovering or focusing one lights the
// matching month cells gold — same read-path pattern as the dashboard
// mini-calendar jump, zero round trips.
{
  const light = (element, on) => {
    for (const iso of (element.dataset.dates || "").split(",")) {
      for (const cell of document.querySelectorAll(`.month-day[data-date="${iso}"]`)) {
        cell.classList.toggle("is-class-hover", on);
      }
    }
  };
  const from = (event) => event.target.closest?.("[data-dates]");
  addEventListener("mouseover", (event) => { const el = from(event); if (el) light(el, true); });
  addEventListener("mouseout", (event) => { const el = from(event); if (el) light(el, false); });
  addEventListener("focusin", (event) => { const el = from(event); if (el) light(el, true); });
  addEventListener("focusout", (event) => { const el = from(event); if (el) light(el, false); });
}

// Honest status polling (documents): rows carrying data-poll re-fetch
// their server-rendered fragment and swap it in — the status shown is
// always the real backend row, never a predicted one. Terminal rows render
// without data-poll, so polling stops by itself. Pure enhancement: with JS
// off, a reload shows the same truth. The interval always runs (a page
// with no data-poll rows costs an empty query) so rows reached via the
// shell-persistent nav swap still poll.
{
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
      body: new URLSearchParams(new FormData(form)),
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

// Theme toggle enhancement (ADR-14): flip data-theme instantly, persist
// via the same POST the JS-off form would make. The server-stamped
// attribute remains the source of truth on the next full page load.
addEventListener(
  "submit",
  (event) => {
    const form = event.target.closest("[data-theme-form]");
    if (!form) return;
    const choice = event.submitter?.value;
    if (!choice) return;
    event.preventDefault();
    if (choice === "system") {
      document.documentElement.removeAttribute("data-theme");
    } else {
      document.documentElement.setAttribute("data-theme", choice);
    }
    for (const button of document.querySelectorAll("[data-theme-form] button")) {
      button.setAttribute("aria-pressed", String(button.value === choice));
    }
    const body = new URLSearchParams(new FormData(form));
    body.set("theme", choice);
    fetch(form.action, { method: "POST", body, credentials: "same-origin" }).catch(() => {});
  },
  true,
);

// Catalog live search (read path, FRONTEND.md §4): debounce typing, then
// fetch the SERVER-filtered results fragment and swap it in — same truth
// as the GET form floor (ADR-15), sooner. The region dims via aria-busy
// only while the request is actually in flight.
let searchTimer = null;
let searchAbort = null;

addEventListener("input", (event) => {
  const form = event.target.closest("[data-live-search]");
  if (!form) return;
  clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    const region = document.querySelector("[data-search-results]");
    if (!region) return;
    const url = `${form.action}?${new URLSearchParams(new FormData(form))}`;
    region.setAttribute("aria-busy", "true");
    searchAbort?.abort();
    searchAbort = new AbortController();
    try {
      const response = await fetch(url, {
        headers: { "X-Fragment": "results" },
        credentials: "same-origin",
        signal: searchAbort.signal,
      });
      const html = (await response.text()).trim();
      if (!response.ok || !html.startsWith("<div")) {
        region.removeAttribute("aria-busy");
        return;
      }
      region.outerHTML = html;
      const fresh = document.querySelector("[data-search-results]");
      const count = document.querySelector("[data-search-count]");
      if (count && fresh?.dataset.count) count.textContent = fresh.dataset.count;
      // Real URLs (FRONTEND.md §5): reloading or sharing keeps the query.
      history.replaceState(null, "", url);
    } catch {
      // Aborted by newer input, or offline — the GET form still works.
      document.querySelector("[data-search-results]")?.removeAttribute("aria-busy");
    }
  }, 275);
});

// Rail collapse/expand enhancement (ADR-16): flip the shell class
// instantly, persist via the same POST the JS-off form would make.
addEventListener(
  "submit",
  (event) => {
    const form = event.target.closest("[data-rail-form]");
    if (!form) return;
    event.preventDefault();
    const collapsing = event.submitter?.value === "collapsed";
    document.querySelector(".shell")?.classList.toggle("rail-collapsed", collapsing);
    const button = form.querySelector("button[name=rail]");
    button.value = collapsing ? "expanded" : "collapsed";
    const label = collapsing ? "Expand navigation" : "Collapse navigation";
    button.setAttribute("aria-label", label);
    button.title = label;
    button.setAttribute("aria-expanded", String(!collapsing));
    const body = new URLSearchParams(new FormData(form));
    body.set("rail", collapsing ? "collapsed" : "expanded");
    fetch(form.action, { method: "POST", body, credentials: "same-origin" }).catch(() => {});
  },
  true,
);

window.Alpine = Alpine;
Alpine.start();
