// Material-style interaction feedback, wired once at the document level so no
// component markup has to change:
//   • Click ripple — a circle that expands from the cursor on buttons, icon
//     buttons, nav links, the brand, and session cards (Polymer paper-ripple
//     style). The ink uses `currentColor`, so it adapts to each surface.
//   • Text fields — on pointer-down we record where the click landed as a CSS
//     custom property (`--ripple-x`) so the Material focus underline grows from
//     that point instead of always from the centre.
// Honors prefers-reduced-motion: no ripple is spawned when the user opts out.

const REDUCED = window.matchMedia("(prefers-reduced-motion: reduce)");

// Elements that get an ink ripple. They are given position:relative + overflow:
// hidden in CSS so the ripple is clipped to their shape.
const RIPPLE_SELECTOR = ".btn, .icon-btn, .crumb, .ctx-menu button, .brand";
// Form controls whose focus underline should originate at the click point.
const FIELD_SELECTOR = "input, textarea, select";

function isDisabled(el) {
  return el.hasAttribute("disabled") || el.getAttribute("aria-disabled") === "true";
}

/** Spawn one ripple circle inside *el*, centred on the (clientX, clientY) point. */
function spawnRipple(el, clientX, clientY) {
  const rect = el.getBoundingClientRect();
  if (!rect.width || !rect.height) {
    return;
  }
  const x = clientX - rect.left;
  const y = clientY - rect.top;
  // Diameter = twice the distance to the farthest corner, so the circle always
  // covers the whole element regardless of where it was clicked.
  const reachX = Math.max(x, rect.width - x);
  const reachY = Math.max(y, rect.height - y);
  const diameter = 2 * Math.hypot(reachX, reachY);

  const ink = document.createElement("span");
  ink.className = "ripple-ink";
  ink.style.width = ink.style.height = `${diameter}px`;
  ink.style.left = `${x - diameter / 2}px`;
  ink.style.top = `${y - diameter / 2}px`;
  ink.addEventListener("animationend", () => ink.remove(), { once: true });
  el.appendChild(ink);
}

function onPointerDown(event) {
  // Primary button / touch / pen only.
  if (typeof event.button === "number" && event.button !== 0) {
    return;
  }
  const target = event.target;
  if (!(target instanceof Element)) {
    return;
  }

  // Text field: set the underline growth origin as a percentage of the width.
  const field = target.closest(FIELD_SELECTOR);
  if (field) {
    const rect = field.getBoundingClientRect();
    if (rect.width) {
      const pct = ((event.clientX - rect.left) / rect.width) * 100;
      field.style.setProperty("--ripple-x", `${Math.max(0, Math.min(100, pct))}%`);
    }
    return;
  }

  if (REDUCED.matches) {
    return;
  }
  const el = target.closest(RIPPLE_SELECTOR);
  if (el && !isDisabled(el)) {
    spawnRipple(el, event.clientX, event.clientY);
  }
}

/** Attach the global pointer-down listener. Call once, after the app mounts. */
export function initRipple() {
  document.addEventListener("pointerdown", onPointerDown, { passive: true });
}
