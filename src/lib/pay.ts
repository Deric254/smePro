// The one place that knows how a customer pays the owner for a licence: the plans, the PayPal
// account, the payment reference and the proof email. The pay window (components/PayWindow.tsx)
// is the only screen that uses it, and `openLink` is the only way the app opens these links.

export const PAY = {
  email: 'dericmarangu@gmail.com',
  paypal: 'https://www.paypal.com/myaccount/transfer/homepage',
  app: 'smePro',
  tag: 'SME',
  plans: {
    M: { name: 'monthly', label: '$100 / month', amount: 100 },
    L: { name: 'lifetime', label: '$1000 for life', amount: 1000 },
  },
} as const;

export type Plan = keyof typeof PAY.plans;

// The only links the app ever opens in the person's own browser or email app. The same list is
// the scope of the opener permission in src-tauri/capabilities/default.json.
export const LINK_ALLOW = [
  'https://dericbi.vercel.app/renew.html?',
  'https://www.paypal.com/',
  'https://mail.google.com/mail/?view=cm',
  `mailto:${PAY.email}?`,
] as const;

export function linkAllowed(url: string): boolean {
  return url.length <= 4000 && LINK_ALLOW.some((prefix) => url.startsWith(prefix));
}

// Same app + computer + plan always gives the same reference, so a repeat payment is easy to spot.
export function payRef(code: string, plan: Plan): string {
  return `LIC-${PAY.tag}-${code.replace(/[^A-Z0-9]/gi, '').toUpperCase()}-${plan}`;
}

export function payMail(code: string, plan: Plan): { mailto: string; gmail: string } {
  const p = PAY.plans[plan];
  const ref = payRef(code, plan);
  const subject = `Payment proof - ${PAY.app} licence ${p.name} - ${ref}`;
  const body = [
    'Hello Deric,', '',
    'I have paid by PayPal. My proof is attached.', '',
    `What I paid for: ${PAY.app} licence, ${p.name}`,
    `Amount: $${p.amount} USD`,
    `Reference: ${ref}`,
    `Activation code: ${code}`,
    'PayPal transaction ID: ',
    'My name: ', '',
    '(Please attach your PayPal screenshot before sending.)',
  ].join('\n');
  const q = (s: string) => encodeURIComponent(s);
  return {
    mailto: `mailto:${PAY.email}?subject=${q(subject)}&body=${q(body)}`,
    gmail: `https://mail.google.com/mail/?view=cm&fs=1&to=${q(PAY.email)}&su=${q(subject)}&body=${q(body)}`,
  };
}

// Remembered on this computer only, per reference, so the next visit says the proof was already sent.
const sentKey = (ref: string) => `smepro.paysent.${ref}`;

export function proofSentAt(ref: string): number | null {
  try {
    const v = localStorage.getItem(sentKey(ref));
    return v ? Number(v) : null;
  } catch { return null; }
}

export function rememberProofSent(ref: string): void {
  try { localStorage.setItem(sentKey(ref), String(Date.now())); } catch { /* not remembered; the window still works */ }
}

// Opens an allowed link with Tauri's opener (a plain browser tab only when running outside Tauri, in development).
export async function openLink(url: string): Promise<boolean> {
  if (!linkAllowed(url)) return false;
  try {
    if ('__TAURI_INTERNALS__' in window) {
      const { openUrl } = await import('@tauri-apps/plugin-opener');
      await openUrl(url);
    } else {
      window.open(url, '_blank', 'noopener');
    }
    return true;
  } catch {
    return false;
  }
}
