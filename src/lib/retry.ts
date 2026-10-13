import { ApiError } from '../api';

// On a slower-starting device — confirmed concretely on Android, not
// theorized: screenshots showed "Could not load modules" plus a
// business name and account badge stuck on their loading placeholder
// ("…") forever, on the very first screen after a cold start — this
// app's own embedded HTTP server can still be finishing its own
// startup (spawning its thread, binding the listening socket, opening
// the database) at the exact moment the very first screen already
// needs data from it. That's a real, one-time race at cold start, not
// a general reliability problem: the first fetch can lose it outright
// — not a slow response, an actual "nothing is listening on this port
// yet" connection failure — and every one of the three places this
// was happening (the module list, the business name, the current
// user) fired once on mount with no retry at all, so losing that race
// even once left the UI stuck on a placeholder that was only ever
// meant to be shown for a moment.
//
// This wraps exactly those startup fetches with a few short, automatic
// retries — giving a slow-starting local server the extra half-second
// to a few seconds it sometimes genuinely needs on a cold start. It is
// NOT a substitute for real error handling, and it only retries a
// CONNECTION failing outright (nothing answered at all). An ApiError
// means the server DID answer — with an error, but it answered — which
// is never the "still starting up" case this exists for; retrying that
// blindly would only hide a real problem for a few extra seconds
// instead of fixing anything, so it's rethrown immediately instead.
//
// BUDGET: the server needs to be given up to ~19 seconds to come up —
// when the CPU, disk or RAM is saturated (or antivirus is scanning the
// freshly launched binary), opening the encrypted database can take far
// longer than on an idle machine, and an early give-up landed
// permanently on "Could not load modules". Polling is frequent (100 ms,
// growing to a 400 ms cap) because the delay is also dead time the user
// waits through: with the old 400 ms–1.5 s steps, a server that came up
// just after an attempt failed sat unused for up to 1.5 s before the
// next try. A server that is up costs nothing: the first attempt wins. A
// genuinely dead server is still reported, just not mistaken for a slow
// one.
export async function retryOnConnectionFailure<T>(
  fn: () => Promise<T>,
  attempts = 50,
  delayMs = 100,
  maxDelayMs = 400,
): Promise<T> {
  let lastErr: unknown;
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (err) {
      lastErr = err;
      if (err instanceof ApiError) throw err;
      if (i < attempts - 1) {
        await new Promise((r) => setTimeout(r, Math.min(delayMs * (i + 1), maxDelayMs)));
      }
    }
  }
  throw lastErr;
}
