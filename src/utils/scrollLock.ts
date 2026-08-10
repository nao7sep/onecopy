// Reference-counted background scroll lock.
//
// While any modal is open, the body must not scroll behind the backdrop. With
// stacked modals (e.g. a dirty-close confirmation over Settings), the lock is
// held until the last modal closes, so closing the inner one does not unlock
// the page underneath the outer one. The original overflow is captured on the
// first acquire and restored on the last release.

let lockCount = 0;
let previousOverflow = "";

export function acquireScrollLock(): void {
  if (lockCount === 0) {
    previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
  }
  lockCount += 1;
}

export function releaseScrollLock(): void {
  if (lockCount === 0) {
    return;
  }
  lockCount -= 1;
  if (lockCount === 0) {
    document.body.style.overflow = previousOverflow;
  }
}
