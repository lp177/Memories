// Backdrop-dismiss for native <dialog> modals.
//
// A dialog opened with showModal() already closes on Escape; these helpers add
// the matching "click outside to dismiss" behaviour. Because `dialog.mdl` has
// zero padding, a click whose target is the <dialog> element itself landed on
// the ::backdrop, not on the content. We require the press to both start AND
// end on the backdrop so that a text selection dragged out of the dialog does
// not accidentally close it. Only wire these onto dialogs that are cancelable
// (i.e. have a Close/Cancel affordance) — never onto a forced-choice modal.

let pressedBackdrop = false;

export function modalPress(event) {
  pressedBackdrop = event.target === event.currentTarget;
}

export function modalBackdropClose(event) {
  if (pressedBackdrop && event.target === event.currentTarget) {
    event.currentTarget.close();
  }
  pressedBackdrop = false;
}
