const API_BASE = 'http://127.0.0.1:8080';

let authToken: string | null = localStorage.getItem('erp_token');
let businessId: string | null = localStorage.getItem('erp_business_id');

export function setSession(token: string, biz: string) {
  authToken = token;
  businessId = biz;
  localStorage.setItem('erp_token', token);
  localStorage.setItem('erp_business_id', biz);
}

export function clearSession() {
  authToken = null;
  businessId = null;
  localStorage.removeItem('erp_token');
  localStorage.removeItem('erp_business_id');
}

export function hasSession() {
  return !!authToken;
}

export function getToken() {
  return authToken;
}

export function getBusinessId() {
  return businessId;
}

class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function request(path: string, options: RequestInit = {}, needsBusinessId = false) {
  const headers: Record<string, string> = { 'Content-Type': 'application/json', ...(options.headers as any) };
  if (authToken) headers['Authorization'] = `Bearer ${authToken}`;
  if (needsBusinessId && businessId) headers['X-Business-Id'] = businessId;

  const res = await fetch(`${API_BASE}${path}`, { ...options, headers });
  if (!res.ok) {
    let message = `Request failed (${res.status})`;
    try {
      const body = await res.json();
      message = body.error || message;
    } catch {
      /* non-JSON error body, keep default message */
    }
    throw new ApiError(res.status, message);
  }
  const contentType = res.headers.get('content-type') || '';
  if (contentType.includes('application/json')) return res.json();
  return res.blob();
}

export { ApiError };

// ---- First-run setup ----
export const getSetupStatus = () =>
  fetch(`${API_BASE}/setup/status`).then((res) => res.json());

export const getResolvedBusinessId = (): Promise<{ business_id: string | null }> =>
  fetch(`${API_BASE}/setup/business-id`).then((res) => res.json());

export const createBusiness = (payload: Record<string, string>) =>
  fetch(`${API_BASE}/setup/create-business`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }).then(async (res) => {
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, body.error || 'Could not create business');
    return body;
  });

// ---- Auth ----
export const logout = () => request('/auth/logout', { method: 'POST' });

export const login = (username: string, password: string, biz: string) =>
  fetch(`${API_BASE}/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Business-Id': biz },
    body: JSON.stringify({ username, password }),
  }).then(async (res) => {
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new ApiError(res.status, body.error || 'Login failed');
    }
    return res.json();
  });

export const login2fa = (tempToken: string, code: string) =>
  fetch(`${API_BASE}/auth/2fa/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ temp_token: tempToken, code }),
  }).then(async (res) => {
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, body.error || '2FA verification failed');
    return body;
  });

export const recoverViaSecurityQuestions = (biz: string, payload: Record<string, string>) =>
  fetch(`${API_BASE}/auth/recover/security-questions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Business-Id': biz },
    body: JSON.stringify(payload),
  }).then(async (res) => {
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, body.error || 'Recovery failed');
    return body;
  });

export const recoverViaAdminCode = (biz: string, payload: Record<string, string>) =>
  fetch(`${API_BASE}/auth/recover/admin-code`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Business-Id': biz },
    body: JSON.stringify(payload),
  }).then(async (res) => {
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, body.error || 'Recovery failed');
    return body;
  });

// ---- License ----
export const getLicenseStatus = () => request('/license/status');
export const activateLicense = () => request('/license/activate', { method: 'POST' });
export const payLicense = () => request('/license/pay', { method: 'POST' });

// ---- Modules ----
export const getBusinessInfo = () => request('/business');
export const listModules = () => request('/modules');
export const enableModule = (moduleId: string) => request(`/modules/${moduleId}/enable`, { method: 'POST' });

// ---- Point of sale — atomically links Sales and Inventory (and,
// optionally, Debt & Credit for a sale on credit). See pos.rs. ----
export interface CartItem { inventory_record_id: string; quantity: number }
export interface CheckoutRequest {
  items: CartItem[];
  payment_method?: string;
  customer?: string;
  allow_oversell?: boolean;
  on_credit?: boolean;
  due_date?: string;
}
export const checkout = (req: CheckoutRequest) =>
  request('/pos/checkout', { method: 'POST', body: JSON.stringify(req) });
export const getOrder = (orderId: string) => request(`/pos/orders/${orderId}`);

// ---- Refunds — the counterpart to checkout. See refund.rs. ----
export interface RefundRequest {
  sale_id: string;
  quantity: number;
  refund_amount: number;
  reason?: string;
  restock?: boolean;
}
export const processRefund = (req: RefundRequest) =>
  request('/sales/refund', { method: 'POST', body: JSON.stringify(req) });

// ---- Receiving stock — the buying-side counterpart. See receiving.rs. ----
export const receiveStock = (purchaseRecordId: string, quantityReceived?: number) =>
  request('/purchasing/receive', {
    method: 'POST',
    body: JSON.stringify({ purchase_record_id: purchaseRecordId, quantity_received: quantityReceived }),
  });

// ---- Repacking / breaking bulk. See repack.rs. ----
export const repackStock = (req: {
  source_record_id: string; source_quantity: number;
  target_record_id: string; target_quantity_produced: number; notes?: string;
}) => request('/inventory/repack', { method: 'POST', body: JSON.stringify(req) });
export const getModuleSchema = (moduleId: string) => request(`/modules/${moduleId}/schema`);
export const listRecords = (moduleId: string, search?: string) =>
  request(`/modules/${moduleId}/records${search ? `?search=${encodeURIComponent(search)}` : ''}`);
export const createRecord = (moduleId: string, data: Record<string, unknown>) =>
  request(`/modules/${moduleId}/records`, { method: 'POST', body: JSON.stringify(data) });
export const updateRecord = (moduleId: string, id: string, data: Record<string, unknown>) =>
  request(`/modules/${moduleId}/records/${id}`, { method: 'PUT', body: JSON.stringify(data) });
export const deleteRecord = (moduleId: string, id: string) =>
  request(`/modules/${moduleId}/records/${id}`, { method: 'DELETE' });

export const exportModule = async (moduleId: string) => {
  const blob = await request(`/modules/${moduleId}/export`);
  downloadBlob(blob, `${moduleId}_export.xlsx`);
};

// ---- Reports ----
export const runReport = (moduleId: string, params: Record<string, string>) =>
  request(`/modules/${moduleId}/report?${new URLSearchParams(params)}`);
export const exportReport = async (moduleId: string, params: Record<string, string>) => {
  const blob = await request(`/modules/${moduleId}/report/export?${new URLSearchParams(params)}`);
  downloadBlob(blob, `${moduleId}_report.xlsx`);
};
export const runForecast = (moduleId: string, params: Record<string, string>) =>
  request(`/modules/${moduleId}/forecast?${new URLSearchParams(params)}`);

function downloadBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

// ---- AI ----
export const askAi = (question: string) =>
  request('/ai/ask', { method: 'POST', body: JSON.stringify({ question }) });
export const getAiContext = () => request('/ai/context');

// (Notifications — see the fuller, typed versions further down:
// listNotifications, sendNotification, sendLowStockAlert)

// ---- Roles & permissions ----
export const listRoles = () => request('/roles');
export const createRole = (name: string) =>
  request('/roles', { method: 'POST', body: JSON.stringify({ name }) });
export const deleteRole = (roleId: string) =>
  request(`/roles/${roleId}`, { method: 'DELETE' });
export const setRoleAdminFlag = (roleId: string, canAdminister: boolean) =>
  request(`/roles/${roleId}/admin-flag`, { method: 'PUT', body: JSON.stringify({ can_administer: canAdminister }) });
export const getRolePermissions = (roleId: string) => request(`/roles/${roleId}/permissions`);
export const setRolePermissions = (roleId: string, moduleId: string, actions: string[]) =>
  request(`/roles/${roleId}/permissions`, { method: 'PUT', body: JSON.stringify({ module_id: moduleId, actions }) });

// ---- Users ----
export const listUsers = () => request('/users');
export const createUser = (payload: {
  username: string; password: string; role_id: string;
  security_q1: string; security_a1: string; security_q2: string; security_a2: string;
}) => request('/users', { method: 'POST', body: JSON.stringify(payload) });
export const setUserRole = (userId: string, roleId: string) =>
  request(`/users/${userId}/role`, { method: 'PUT', body: JSON.stringify({ role_id: roleId }) });
export const deactivateUser = (userId: string) =>
  request(`/users/${userId}`, { method: 'DELETE' });

// ---- Units & currencies ----
export const listUnits = () => request('/units');
export const createUnit = (name: string, abbreviation?: string) =>
  request('/units', { method: 'POST', body: JSON.stringify({ name, abbreviation }) });
export const deleteUnit = (unitId: string) => request(`/units/${unitId}`, { method: 'DELETE' });

export const listCurrencies = () => request('/currencies');
export const createCurrency = (code: string, symbol?: string, name?: string) =>
  request('/currencies', { method: 'POST', body: JSON.stringify({ code, symbol, name }) });
export const deleteCurrency = (currencyId: string) => request(`/currencies/${currencyId}`, { method: 'DELETE' });

// ---- Settings (theme, locale, etc.) ----
export const getSettings = () => request('/settings');
export const setSetting = (key: string, value: string) =>
  request('/settings', { method: 'PUT', body: JSON.stringify({ key, value }) });

// ---- Notifications ----
export interface NotificationRecord {
  id: string;
  channel: string;
  recipient: string;
  message: string;
  status: string;
  created_at: string;
}
export const listNotifications = (): Promise<{ notifications: NotificationRecord[] }> => request('/notifications');
export const sendNotification = (channel: 'whatsapp' | 'sms', recipient: string, message: string) =>
  request('/notifications/send', { method: 'POST', body: JSON.stringify({ channel, recipient, message }) });
export const sendLowStockAlert = (channel: 'whatsapp' | 'sms', recipient: string) =>
  request('/notifications/low-stock-alert', { method: 'POST', body: JSON.stringify({ channel, recipient }) });

// ---- OCR photo import — photograph a paper ledger page, review the
// guessed records, confirm to actually create them. Requires
// tesseract-ocr installed on this machine (not bundled with the app) —
// callers should show a clear, specific message on failure rather than
// a generic error, since "not installed" is the overwhelmingly likely
// cause for most people trying this. ----
export const ocrExtractText = (imageBase64: string): Promise<{ raw_text: string }> =>
  request('/import/ocr/extract', { method: 'POST', body: JSON.stringify({ image_base64: imageBase64 }) });
export const ocrParseCandidates = (moduleId: string, rawText: string): Promise<{ candidates: Record<string, unknown>[] }> =>
  request('/import/ocr/parse', { method: 'POST', body: JSON.stringify({ module_id: moduleId, raw_text: rawText }) });
export const bulkCreateRecords = (moduleId: string, records: Record<string, unknown>[]): Promise<{ created: number; errors: { index: number; error: string }[] }> =>
  request(`/modules/${moduleId}/records/bulk`, { method: 'POST', body: JSON.stringify({ records }) });

// ---- Change business type after setup — re-applies that type's
// sensible default module set. ----
export const changeBusinessType = (businessType: string): Promise<{ enabled_modules: string[] }> =>
  request('/onboarding/setup', { method: 'POST', body: JSON.stringify({ business_type: businessType }) });

// ---- Audit log — Owner-only, the record of who did what and when.
// This is the actual "nothing gets lost, everything is accountable"
// guarantee made concrete: every write anywhere in the app is logged
// here automatically, not opt-in. ----
export interface AuditLogEntry {
  id: string;
  user_id: string | null;
  module_id: string;
  action: string;
  record_id: string | null;
  details: unknown;
  timestamp: string;
}
export const getAuditLog = (moduleId?: string, limit = 200): Promise<{ entries: AuditLogEntry[] }> =>
  request(`/audit-log?limit=${limit}${moduleId ? `&module_id=${encodeURIComponent(moduleId)}` : ''}`);

// ---- Backup & restore — real disaster recovery, not a suggestion to
// copy files manually. The raw database key is never shipped in the
// backup itself; a passphrase the owner chooses wraps it instead, so
// possessing the backup file alone is never enough to open it. ----
export interface BackupData {
  database_base64: string;
  wrapped_key_base64: string;
  created_at: string;
  schema_version: string;
}
export const createBackup = (passphrase: string): Promise<BackupData> =>
  request('/admin/backup', { method: 'POST', body: JSON.stringify({ passphrase }) });
export const restoreBackup = (data: { database_base64: string; wrapped_key_base64: string; passphrase: string }) =>
  request('/admin/restore', { method: 'POST', body: JSON.stringify(data) });
export const restoreBackupFreshInstall = (data: { database_base64: string; wrapped_key_base64: string; passphrase: string }) =>
  request('/setup/restore', { method: 'POST', body: JSON.stringify(data) });

// ---- Real payment collection — Stripe checkout / M-Pesa STK push.
// Separate from license/activate and license/pay, which are trust-based
// manual toggles with no actual charge — these two call real payment
// providers and money actually moves. ----
export interface PaymentHistoryEntry {
  provider: string;
  reference: string;
  purpose: string;
  amount: number;
  currency: string;
  status: string;
  created_at: string;
  completed_at: string | null;
}
export const getPaymentHistory = (): Promise<{ payments: PaymentHistoryEntry[] }> => request('/payments/history');
export const initiateStripeCheckout = (purpose: 'activation' | 'subscription', amount: number, currency: string) =>
  request('/payments/checkout', { method: 'POST', body: JSON.stringify({ provider: 'stripe', purpose, amount, currency }) });
export const initiateMpesaPayment = (purpose: 'activation' | 'subscription', amount: number, phone: string) =>
  request('/payments/checkout', { method: 'POST', body: JSON.stringify({ provider: 'mpesa', purpose, amount, phone }) });

// ---- Vendor license key redemption ----
export const getVendorLicenseStatus = () => request('/license/vendor/status');
export const redeemVendorKey = (key: string) =>
  request('/license/vendor/redeem', { method: 'POST', body: JSON.stringify({ key }) });
