// Behavior check for the dashboard smart calendar: focusing an event row
// jumps the grid to that event's month and lights its dates; blurring
// returns to the current month with today marked. Runs app.js inside
// jsdom against the rendered dashboard page — the same enhancement path a
// browser takes (focusin/focusout mirror the hover pair).
import { readFileSync, readdirSync } from "node:fs";
import { JSDOM } from "jsdom";

const html = readFileSync("test/pages/dashboard.html", "utf8");
const [bundle] = readdirSync("dist").filter(
  (name) => name.startsWith("app-") && name.endsWith(".js"),
);
const script = readFileSync(`dist/${bundle}`, "utf8");

// A real origin so the bundle's localStorage access works under jsdom.
const { window } = new JSDOM(html, {
  runScripts: "outside-only",
  url: "http://localhost/",
});
window.eval(script);
const { document } = window;

function assert(condition, message) {
  if (!condition) {
    console.error(`FAIL calendar: ${message}`);
    process.exit(1);
  }
}

const tables = [...document.querySelectorAll("[data-cal-month]")];
assert(tables.length >= 2, "sample renders the current month plus an event month");
assert(!tables[0].hidden && tables.slice(1).every((t) => t.hidden), "only the current month starts visible");
assert(document.querySelector(".is-today[aria-current='date']"), "today is marked in the baseline grid");

// The far event (Mid-semester examinations) lives in a hidden month.
const far = [...document.querySelectorAll("[data-event]")].find(
  (li) => li.dataset.from.slice(0, 7) !== tables[0].dataset.calMonth,
);
assert(far, "sample has an event outside the current month");
assert(far.tabIndex === 0, "event rows are keyboard focusable");

far.dispatchEvent(new window.FocusEvent("focusin", { bubbles: true }));
const target = tables.find((t) => t.dataset.calMonth === far.dataset.from.slice(0, 7));
assert(!target.hidden, "focus jumps the grid to the event's month");
assert(tables[0].hidden, "the current month yields while jumped");
assert(document.querySelector("[data-minical]").classList.contains("is-jumped"), "the month name is flagged as jumped");
const hot = [...target.querySelectorAll(".cal-hot")];
assert(hot.length >= 1, "the event's dates are highlighted");
assert(
  hot.every((d) => far.dataset.from <= d.dataset.date && d.dataset.date <= far.dataset.to),
  "only dates inside the event span are highlighted",
);

far.dispatchEvent(new window.FocusEvent("focusout", { bubbles: true }));
assert(!tables[0].hidden && target.hidden, "blur returns to the current month");
assert(document.querySelectorAll(".cal-hot").length === 0, "highlights clear on blur");

console.log("ok   calendar jump (focus in/out, span highlight, restore)");

// Schedule-page class-date highlight: hovering/focusing a weekly class
// row lights that class's remaining meeting dates on the month calendar.
{
  const schedHtml = readFileSync("test/pages/schedule.html", "utf8");
  const sched = new JSDOM(schedHtml, { runScripts: "outside-only", url: "http://localhost/ui/schedule" });
  sched.window.eval(script);
  const doc = sched.window.document;
  const row = doc.querySelector("[data-dates]");
  assert(row, "schedule sample has a class row carrying data-dates");
  assert(row.tabIndex === 0, "class rows are keyboard focusable");
  row.dispatchEvent(new sched.window.FocusEvent("focusin", { bubbles: true }));
  const lit = [...doc.querySelectorAll(".month-day.is-class-hover")];
  const dates = row.dataset.dates.split(",");
  assert(lit.length >= 1, "focusing a class row lights its dates on the month grid");
  assert(lit.every((cell) => dates.includes(cell.dataset.date)), "only that class's dates light up");
  row.dispatchEvent(new sched.window.FocusEvent("focusout", { bubbles: true }));
  assert(doc.querySelectorAll(".is-class-hover").length === 0, "highlight clears on blur");
  console.log("ok   schedule class-date highlight (focus in/out, clear)");
}

// Sign-in photo pager: arrows unhide with JS and page the set through
// the two layers.
{
  const signinHtml = readFileSync("test/pages/signin.html", "utf8");
  const signin = new JSDOM(signinHtml, { runScripts: "outside-only", url: "http://localhost/ui/login" });
  signin.window.eval(script);
  const doc = signin.window.document;
  const next = doc.querySelector("[data-photo-next]");
  const prev = doc.querySelector("[data-photo-prev]");
  assert(next && prev, "signin renders both pager arrows");
  assert(!next.hidden && !prev.hidden, "enhancement unhides the arrows");
  const layers = [...doc.querySelectorAll(".auth-photo")];
  assert(layers.length === 2, "two crossfade layers");
  next.dispatchEvent(new signin.window.MouseEvent("click", { bubbles: true }));
  assert(
    layers[1].style.backgroundImage.includes("/assets/auth-02"),
    `next must use an absolute /assets/ URL (relative resolves against /ui/login and 404s), got: ${layers[1].style.backgroundImage}`,
  );
  assert(layers[1].classList.contains("show") && !layers[0].classList.contains("show"), "layers swapped roles");
  prev.dispatchEvent(new signin.window.MouseEvent("click", { bubbles: true }));
  assert(layers[0].style.backgroundImage.includes("auth-01"), "prev returns to the first photo");
  console.log("ok   sign-in photo pager (arrows unhide, next/prev, layer swap)");
}

// The bundles' always-on polling intervals keep the jsdom event loop
// alive; every assertion above has already run.
process.exit(0);
