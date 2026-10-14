// Background internet calls (update checks) must never compete with the
// app starting up: while the window is opening, the local server is
// starting and the first screens are loading their data, a network call
// to GitHub (which can take seconds to time out on a poor connection)
// only adds load and delay. This resolves once the page has fully loaded,
// a grace period after the app launched has passed, and the browser is
// idle — callers `await` it before their first network request.
//
// The clock starts when this module is first loaded (app launch), not
// when the caller mounts, so moving between the login screen and the main
// screen doesn't restart it, and every caller shares the same moment.
const STARTUP_GRACE_MS = 10_000;

const launched = Date.now();

function pageLoaded(): Promise<void> {
  if (document.readyState === 'complete') return Promise.resolve();
  return new Promise((resolve) => window.addEventListener('load', () => resolve(), { once: true }));
}

function idle(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof window.requestIdleCallback === 'function') {
      window.requestIdleCallback(() => resolve(), { timeout: 5_000 });
    } else {
      setTimeout(resolve, 0);
    }
  });
}

export const afterStartup: Promise<void> = pageLoaded()
  .then(() => new Promise<void>((resolve) => setTimeout(resolve, Math.max(0, STARTUP_GRACE_MS - (Date.now() - launched)))))
  .then(idle);
