// A `let`, not a `const` — this is a live ES-module binding, so every
// file that does `import { API_BASE } from '../api'` sees the updated
// value automatically the moment `setApiBase` below reassigns it, with
// no getter function needed. That matters here specifically: this
// device might be a LAN "client" pointed at a different device
// entirely (see network.ts / Admin → Network), and every request in
// the whole app — not just the ones that already went through this
// file's own `request()` helper — needs to follow that, or switching
// modes would silently leave some features still talking to
// 127.0.0.1 while others correctly follow the host.
export let API_BASE = 'http://127.0.0.1:8080';
export function setApiBase(url: string) {
  API_BASE = url.trim().replace(/\/+$/, '');
}

export function apiBaseForHost(address: string): string {
  const trimmed = address.trim().replace(/\/+$/, '');
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

// Branding paths come from the backend's filesystem and are Windows paths
// in the desktop build. Only the filename belongs in the public uploads URL.
export function getLogoUrl(storedPath?: string | null): string | null {
  if (!storedPath) return null;
  const filename = storedPath.replace(/\\/g, '/').split('/').pop();
  return filename ? `${API_BASE}/uploads/${encodeURIComponent(filename)}` : null;
}

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

  // cache: 'no-store' is deliberate, not a default worth leaving
  // implicit — every response here is live business data (sales,
  // stock, customers) that must reflect the instant it's requested.
  // The backend already sends Cache-Control: no-store on every
  // response (see security.rs's security_headers) — this is the
  // second, independent layer of the same guarantee, on the request
  // side rather than relying solely on the server's header being
  // honored by whichever WebView engine happens to be running.
  //
  // A THIRD layer, for GET requests specifically: append a
  // cache-busting query param, making every GET URL unique. This
  // exists because "no-store" is a policy the CACHE has to choose to
  // honor — most do, but Tauri's embedded WebView (WebView2 on
  // Windows, WebKit elsewhere) is a different HTTP stack per platform
  // than a regular desktop browser, with its own historically
  // inconsistent record on respecting cache-control for the local
  // fetch() calls this app makes against its own 127.0.0.1 server —
  // and this app also supports pointing at a REMOTE host over LAN
  // (see API_BASE above), adding a real network path with its own
  // potential caching layers in between. A unique URL per request
  // can't be served from a stale cache entry no matter which layer in
  // that chain didn't honor the header — it doesn't rely on anyone's
  // policy being followed correctly. Harmless on the backend: its own
  // router matches routes by splitting the URL on '?' before looking
  // at the path (see http_api.rs's query_params/route dispatch), so
  // an extra query param it never looks for is silently ignored.
  const method = (options.method ?? 'GET').toUpperCase();
  const url = method === 'GET'
    ? `${API_BASE}${path}${path.includes('?') ? '&' : '?'}_t=${Date.now()}`
    : `${API_BASE}${path}`;
  const res = await fetch(url, { ...options, headers, cache: 'no-store' });
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

export const getPublicBranding = (): Promise<{ name: string | null; logo_url: string | null; slogan: string | null }> =>
  fetch(`${API_BASE}/setup/branding`).then((res) => res.json());

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

export interface CurrentUser {
  username: string;
  role_name: string;
  role_id: string;
  business_name: string;
}
// Who's actually signed in right now — powers the account menu. Every
// authenticated user can call this regardless of RBAC permissions,
// since a user always has the right to know their own username and
// role (see the matching comment on the backend route).
export const getCurrentUser = (): Promise<CurrentUser> => request('/auth/me');

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

export interface SecurityQuestions { question1: string | null; question2: string | null }
export const getSecurityQuestions = (biz: string, username: string): Promise<SecurityQuestions> =>
  // GET, not POST — and cache: 'no-store' explicitly, same reasoning
  // as request()'s own default (see api.ts's top): this is live
  // account data, and a GET request is exactly the kind the WebView's
  // own HTTP cache could otherwise silently answer from memory
  // instead of asking the server again.
  fetch(`${API_BASE}/auth/recover/security-questions?username=${encodeURIComponent(username)}`, {
    headers: { 'X-Business-Id': biz },
    cache: 'no-store',
  }).then(async (res) => {
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, body.error || 'Could not look up security questions');
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

// ---- Modules ----
export const getBusinessInfo = () => request('/business');
export const listModules = () => request('/modules');
export const enableModule = (moduleId: string) => request(`/modules/${moduleId}/enable`, { method: 'POST' });
export const disableModule = (moduleId: string) => request(`/modules/${moduleId}/disable`, { method: 'POST' });
// Every module TYPE that exists (reads the real modules/*.json files
// on disk), each flagged with whether THIS business currently has it
// enabled — unlike listModules() above, which only ever returns
// modules the business has touched at least once, so it can't be used
// to discover something like "Purchasing" as available-but-not-yet-on.
export interface AvailableModule { id: string; display_name: string; enabled: boolean }
export const listAvailableModules = (): Promise<{ modules: AvailableModule[] }> => request('/modules/available');

// ---- Point of sale — atomically links Sales and Inventory (and,
// optionally, Debt & Credit for a sale on credit). See pos.rs. ----
export interface CartItem { inventory_record_id: string; quantity: number }
export interface CheckoutRequest {
  items: CartItem[];
  payment_method?: string;
  customer?: string;
  customer_phone?: string;
  allow_oversell?: boolean;
  on_credit?: boolean;
  due_date?: string;
}
export const checkout = (req: CheckoutRequest) =>
  request('/pos/checkout', { method: 'POST', body: JSON.stringify(req) });
export const getOrder = (orderId: string) => request(`/pos/orders/${orderId}`);

// ---- Service sale — the same atomicity + customer-tracking
// guarantee as checkout above, for businesses with no Inventory
// module. See pos::create_service_sale. ----
export interface ServiceLineRequest { description: string; unit_price: number; quantity: number }
export interface ServiceSaleRequest {
  lines: ServiceLineRequest[];
  payment_method?: string;
  customer?: string;
  customer_phone?: string;
}
export const createServiceSale = (req: ServiceSaleRequest) =>
  request('/pos/service-sale', { method: 'POST', body: JSON.stringify(req) });

export interface CustomerSummary {
  id: string; name: string | null; phone: string | null; customer_since: string;
  lifetime_value: number; order_count: number; last_purchase_at: string | null;
}
export interface CustomerDetail extends CustomerSummary {
  purchases: { item_name: string; quantity: number; revenue: number; order_id: string | null; date: string }[];
}
export const listCustomers = (): Promise<{ customers: CustomerSummary[] }> => request('/customers');
export const getCustomer = (id: string): Promise<CustomerDetail> => request(`/customers/${id}`);
export interface CustomerMatch { id: string; name: string | null; phone: string | null }
export const searchCustomers = (query: string): Promise<{ customers: CustomerMatch[] }> =>
  request(`/customers/search?q=${encodeURIComponent(query)}`);

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

// ---- Stock Take: initiate -> count -> close. See stock_take.rs. ----
export interface StockTakeItem {
  id: string;
  inventory_record_id: string;
  item_name: string;
  expected_qty: number;
  counted_qty: number | null;
}
export interface StockTake {
  id: string;
  status: 'in_progress' | 'closed';
  created_at: string;
  closed_at: string | null;
  items: StockTakeItem[];
}
export interface StockTakeSummary {
  id: string;
  status: 'in_progress' | 'closed';
  created_at: string;
  closed_at: string | null;
  item_count: number;
  counted_count: number;
}
export interface StockTakeCloseResult {
  stock_take_id: string;
  items_counted: number;
  items_skipped: number;
  total_variance_units: number;
  adjustments: { inventory_record_id: string; item_name: string; expected_qty: number; counted_qty: number; variance: number }[];
  skipped: { inventory_record_id: string; item_name: string; expected_qty: number }[];
}
export const initiateStockTake = (): Promise<StockTake> =>
  request('/inventory/stocktake/initiate', { method: 'POST' });
export const getOpenStockTake = (): Promise<{ open: StockTake | null }> =>
  request('/inventory/stocktake/open');
export const getStockTake = (id: string): Promise<StockTake> =>
  request(`/inventory/stocktake/${id}`);
export const getStockTakeHistory = (): Promise<{ stock_takes: StockTakeSummary[] }> =>
  request('/inventory/stocktake/history');
export const recordStockTakeCount = (stockTakeId: string, itemId: string, countedQty: number) =>
  request('/inventory/stocktake/count', {
    method: 'POST',
    body: JSON.stringify({ stock_take_id: stockTakeId, item_id: itemId, counted_qty: countedQty }),
  });
export const closeStockTake = (stockTakeId: string): Promise<StockTakeCloseResult> =>
  request(`/inventory/stocktake/${stockTakeId}/close`, { method: 'POST' });

// ---- Settling a debt/credit record. See debt_settlement.rs. ----
export interface SettleDebtSummary {
  debt_record_id: string; party_name: string; direction: string; amount: number;
  settled: true; payment_method: string; posted_to_bookkeeping_as: 'income' | 'expense' | null;
}
// payment_method is required — the backend rejects a blank one (see
// debt_settlement::settle). A settlement is a real cash event; how it
// was paid is a known fact by the time anyone is settling it, not
// something that should ever land in the ledger as "(not set)".
export const settleDebt = (debtRecordId: string, paymentMethod: string): Promise<SettleDebtSummary> =>
  request('/debt_credit/settle', { method: 'POST', body: JSON.stringify({ debt_record_id: debtRecordId, payment_method: paymentMethod }) });
export interface DebtSummary {
  owed_to_business_unpaid: number;
  owed_to_business_unpaid_count: number;
  owed_by_business_unpaid: number;
  owed_by_business_unpaid_count: number;
  overdue_amount: number;
  overdue_count: number;
  due_soon_amount: number;
  due_soon_count: number;
}
// Real totals over the WHOLE Debt & Credit table (see
// debt_settlement::summary on the backend) — deliberately not derived
// from listRecords('debt_credit'), which caps at 1000 rows and would
// silently undercount for a business with more open debt than that.
export const getDebtSummary = (): Promise<DebtSummary> => request('/debt_credit/summary');
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

// ---- Excel import: download a blank template with real field names
// as headers, fill it in (or export existing records and edit them —
// this is also how a stock take works: export, correct the counted
// quantities, reimport), then upload it back. See excel_import.rs.
export const downloadImportTemplate = async (moduleId: string) => {
  const blob = await request(`/modules/${moduleId}/import-template`);
  downloadBlob(blob, `${moduleId}_import_template.xlsx`);
};
export interface ImportExcelResult {
  created: number;
  updated: number;
  errors: { row: number; error: string }[];
}
export const importExcel = (moduleId: string, fileBase64: string, keyField?: string): Promise<ImportExcelResult> =>
  request(`/modules/${moduleId}/import-excel`, {
    method: 'POST',
    body: JSON.stringify({ file_base64: fileBase64, key_field: keyField }),
  });

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
export interface BusinessPulse {
  has_data: boolean;
  revenue_this_period_cents: number;
  revenue_last_period_cents: number;
  pct_change: number | null;
  forecast_next_period_cents: number;
  low_stock_count: number;
  recommendations: string[];
  currency: string;
}
export const askAi = (question: string): Promise<{ answer: string; business_pulse: BusinessPulse }> =>
  request('/ai/ask', { method: 'POST', body: JSON.stringify({ question }) });
export const getAiContext = () => request('/ai/context');

// ---- AI chat history — see ai_chat.rs. Real, persisted sessions
// instead of state that vanished when the panel closed. ----
export interface AiChatSession {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  last_message: string | null;
  message_count: number;
}
export interface AiChatMessage {
  role: 'user' | 'ai';
  content: string;
  created_at: string;
  // Only ever present on a freshly-returned 'ai' message from
  // askAiInSession below — NOT persisted with the message (see
  // http_api.rs's own comment on why: folding this into the stored
  // answer text would pollute future prompts sent back to the AI
  // provider). Reopening a past session's history will show that
  // message without a pulse attached, which is correct, not a bug —
  // the pulse is "how things stand right now," not a historical fact
  // about that exact past moment.
  business_pulse?: BusinessPulse;
}
export const listAiSessions = (): Promise<{ sessions: AiChatSession[] }> => request('/ai/sessions');
export const createAiSession = (): Promise<{ session_id: string }> =>
  request('/ai/sessions', { method: 'POST' });
export const getAiSessionMessages = (sessionId: string): Promise<{ messages: AiChatMessage[] }> =>
  request(`/ai/sessions/${sessionId}/messages`);
export const askAiInSession = (sessionId: string, question: string): Promise<{ answer: string; session_id: string; business_pulse: BusinessPulse }> =>
  request(`/ai/sessions/${sessionId}/ask`, { method: 'POST', body: JSON.stringify({ question }) });
export const clearAiSession = (sessionId: string) =>
  request(`/ai/sessions/${sessionId}/clear`, { method: 'POST' });
export const deleteAiSession = (sessionId: string) =>
  request(`/ai/sessions/${sessionId}`, { method: 'DELETE' });
export const exportAiChatHistory = async () => {
  const blob = await request('/ai/sessions/export.xlsx');
  downloadBlob(blob, 'ai-chat-history.xlsx');
};

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

// ---- Currency conversion — see currency.rs. Rates are cached and
// only refreshed on request (rates_stale tells the UI when it's worth
// nudging the owner to refresh, rather than hitting the external
// exchange-rate API on every page load). Amounts are integer minor
// units (cents) — see money.rs.
export interface CurrencyRate { from_currency: string; to_currency: string; rate: number; fetched_at: number }
export const getCurrencyRates = (base: string): Promise<{ rates: CurrencyRate[]; stale: boolean }> =>
  request(`/currency/rates?base=${encodeURIComponent(base)}`);
export const convertCurrency = (from: string, to: string, amountCents: number): Promise<{ result: number }> =>
  request('/currency/convert', { method: 'POST', body: JSON.stringify({ from, to, amount: amountCents }) });
export const refreshCurrencyRates = (base: string) =>
  request(`/currency/refresh?base=${encodeURIComponent(base)}`, { method: 'POST' });

// ---- Tax rates by category, and a compute preview — see tax.rs.
// IMPORTANT, and surfaced in the UI, not just here: these per-category
// rates are NOT currently applied by real checkouts or invoices —
// pos.rs and invoice.rs both use a single flat business-wide tax rate
// instead. This is a genuine gap in the backend itself, not a UI
// limitation — see the note shown in TaxRatesTab.
export interface TaxRate { category: string; rate: number }
export const listTaxRates = (): Promise<{ rates: TaxRate[] }> => request('/tax/rates');
export const setTaxRate = (category: string, rate: number) =>
  request('/tax/rates', { method: 'POST', body: JSON.stringify({ category, rate }) });
export interface TaxComputeItem { category: string; unit_price: number; quantity: number }
export interface TaxComputeResult {
  subtotal: number;
  total_tax: number;
  total: number;
  tax_inclusive: boolean;
  lines: { category: string; rate: number; taxable_amount: number; tax_amount: number }[];
}
export const computeTax = (items: TaxComputeItem[], taxInclusive: boolean): Promise<TaxComputeResult> =>
  request('/tax/compute', { method: 'POST', body: JSON.stringify({ items, tax_inclusive: taxInclusive }) });

// ---- Settings (theme, locale, etc.) ----
export const getSettings = () => request('/settings');
export const setSetting = (key: string, value: string) =>
  request('/settings', { method: 'PUT', body: JSON.stringify({ key, value }) });

export interface AiSettingsStatus {
  provider: string;
  nvidia_key_set: boolean;
  gemini_key_set: boolean;
  openai_key_set: boolean;
  claude_key_set: boolean;
}
export const getAiSettings = (): Promise<AiSettingsStatus> => request('/ai/settings');

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

export interface NewInvoiceItem { description: string; quantity: number; unit_price: number }
export const createInvoice = (payload: {
  customer: string;
  customer_email?: string;
  customer_phone?: string;
  due_date: string;
  items: NewInvoiceItem[];
  notes?: string;
}) => request('/invoices', { method: 'POST', body: JSON.stringify(payload) });

// ---- Invoice lifecycle — see invoice.rs::transition_status for the
// exact allowed transitions (draft -> sent/cancelled, sent ->
// paid/overdue/cancelled, overdue -> paid). The backend enforces
// these; the UI only needs to know which buttons make sense to show
// for the invoice's current status, not re-implement the rules.
export const markInvoiceSent = (invoiceId: string) => request(`/invoices/${invoiceId}/send`, { method: 'POST' });
export const markInvoicePaid = (invoiceId: string) => request(`/invoices/${invoiceId}/pay`, { method: 'POST' });
export const cancelInvoice = (invoiceId: string) => request(`/invoices/${invoiceId}/cancel`, { method: 'POST' });

// The invoice document itself is frozen at issue time (see
// invoice.rs's own doc comment on why) — this is a separate,
// always-fresh lookup of whatever's been refunded against the sale
// an invoice was auto-generated from, so InvoiceView can disclose it
// without ever rewriting the invoice's own original figures.
export interface InvoiceRefundStatus { refunded_amount: number; is_refunded: boolean }
export const getInvoiceRefundStatus = (invoiceId: string): Promise<InvoiceRefundStatus> =>
  request(`/invoices/${invoiceId}/refund-status`);

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
