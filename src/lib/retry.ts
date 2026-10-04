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
// BUDGET: the original 5 attempts (400/800/1200/1600ms) gave the server
// only ~4 seconds to come up. That is enough on an idle machine but not
// when the CPU, disk or RAM is saturated (or antivirus is scanning the
// freshly launched binary): opening the encrypted database can then take
// longer than that, every retry was spent before the listener existed,
// and the UI landed permanently on "Could not load modules". The delay
// still grows per attempt but is capped, giving ~19 seconds in total —
// a genuinely dead server is still reported, just not mistaken for a
// slow one. A server that is up costs nothing: the first attempt wins.
export async function retryOnConnectionFailure<T>(
  fn: () => Promise<T>,
  attempts = 15,
  delayMs = 400,
  maxDelayMs = 1500,
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
