// Progressive enhancement only — every page works with JavaScript off
// (proven by the test suite, which never executes scripts).
//
// One behavior: after a form is submitted, block a second submission and
// show a busy state, so a double-click or a slow network cannot POST twice
// and the user sees feedback instead of a frozen page. The server-side
// idempotency keys remain the real duplicate guarantee.
"use strict";

addEventListener("submit", function (event) {
  var form = event.target;
  if (form.getAttribute("aria-busy") === "true") {
    event.preventDefault();
    return;
  }
  form.setAttribute("aria-busy", "true");
  var buttons = form.querySelectorAll("button");
  for (var i = 0; i < buttons.length; i++) {
    buttons[i].disabled = true;
  }
});

// Restore forms when a page returns from the back/forward cache.
addEventListener("pageshow", function (event) {
  if (!event.persisted) {
    return;
  }
  var forms = document.querySelectorAll("form[aria-busy]");
  for (var i = 0; i < forms.length; i++) {
    forms[i].removeAttribute("aria-busy");
    var buttons = forms[i].querySelectorAll("button[disabled]");
    for (var j = 0; j < buttons.length; j++) {
      buttons[j].disabled = false;
    }
  }
});
