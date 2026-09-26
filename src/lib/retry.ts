import { ApiError } from '../api';

export async function retryOnConnectionFailure<T>(
  fn: () => Promise<T>,
  attempts = 5,
  delayMs = 400,
): Promise<T> {
  let lastErr: unknown;
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (err) {
      lastErr = err;
      if (err instanceof ApiError) throw err;
      if (i < attempts - 1) {
        await new Promise((r) => setTimeout(r, delayMs * (i + 1)));
      }
    }
  }
  throw lastErr;
}
