// Pay window constants and link allowlist. Run with: npm test
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { LINK_ALLOW, PAY, linkAllowed, payMail, payRef } from '../src/lib/pay.ts';

const CODE = 'ABCD-EFGH-JKLM';

test('the account, plans, tag and app name are the owner\'s', () => {
  assert.equal(PAY.email, 'dericmarangu@gmail.com');
  assert.equal(PAY.paypal, 'https://www.paypal.com/myaccount/transfer/homepage');
  assert.equal(PAY.app, 'smePro');
  assert.equal(PAY.tag, 'SME');
  assert.deepEqual(Object.entries(PAY.plans).map(([k, p]) => [k, p.amount, p.label]), [['M', 100, '$100 / month'], ['L', 1000, '$1000 for life']]);
});

test('the reference is LIC-SME-<activation code, 12 chars>-<M|L>', () => {
  assert.equal(payRef(CODE, 'M'), 'LIC-SME-ABCDEFGHJKLM-M');
  assert.equal(payRef('abcd-efgh-jklm', 'L'), 'LIC-SME-ABCDEFGHJKLM-L');
});

test('the proof email is prefilled and never claims to attach the screenshot', () => {
  const { mailto, gmail } = payMail(CODE, 'L');
  const subject = 'Payment proof - smePro licence lifetime - LIC-SME-ABCDEFGHJKLM-L';
  assert.ok(mailto.startsWith('mailto:dericmarangu@gmail.com?subject='));
  const body = decodeURIComponent(mailto.split('&body=')[1]);
  assert.equal(decodeURIComponent(mailto.split('?subject=')[1].split('&body=')[0]), subject);
  for (const line of ['What I paid for: smePro licence, lifetime', 'Amount: $1000 USD', 'Reference: LIC-SME-ABCDEFGHJKLM-L', `Activation code: ${CODE}`, 'PayPal transaction ID: ', 'My name: ', '(Please attach your PayPal screenshot before sending.)']) {
    assert.ok(body.includes(line), line);
  }
  assert.ok(gmail.startsWith('https://mail.google.com/mail/?view=cm&fs=1&to=dericmarangu%40gmail.com&su='));
  assert.ok(!/auto|attached automatically/i.test(body));
});

test('only the four allowed places can be opened', () => {
  for (const ok of ['https://dericbi.vercel.app/renew.html?app=smepro&code=X', 'https://www.paypal.com/myaccount/transfer/homepage',
    'https://mail.google.com/mail/?view=cm&fs=1&to=a', payMail(CODE, 'M').mailto, payMail(CODE, 'M').gmail]) assert.ok(linkAllowed(ok), ok);
  for (const bad of ['https://evil.example/', 'https://www.paypal.com.evil.example/', 'http://www.paypal.com/', 'mailto:someone@else.com?subject=x',
    'https://dericbi.vercel.app/other.html?x', 'https://dericbi.vercel.app/renew.html', 'file:///etc/passwd', 'javascript:alert(1)', '',
    'https://www.paypal.com/' + 'a'.repeat(4000)]) assert.ok(!linkAllowed(bad), bad);
});

test('the opener permission in the Tauri capability is exactly this allowlist (and the broad default is gone)', () => {
  const cap = JSON.parse(readFileSync(new URL('../src-tauri/capabilities/default.json', import.meta.url), 'utf8'));
  assert.ok(!cap.permissions.includes('opener:default'));
  const scope = cap.permissions.find((p) => p.identifier === 'opener:allow-open-url');
  assert.deepEqual(scope.allow.map((a) => a.url), LINK_ALLOW.map((p) => `${p}*`));
});

test('there is one pay window and one place that opens links', () => {
  const src = (f) => readFileSync(new URL(`../src/${f}`, import.meta.url), 'utf8');
  assert.equal((src('components/PayWindow.tsx').match(/role="dialog"/g) || []).length, 1);
  assert.ok(!/ServiceSale/.test(src('components/PayWindow.tsx')));
});
