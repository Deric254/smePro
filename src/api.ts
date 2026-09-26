export const API_BASE = 'http://127.0.0.1:8080';

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
    }
    throw new ApiError(res.status, message);
  }
  const contentType = res.headers.get('content-type') || '';
  if (contentType.includes('application/json')) return res.json();
  return res.blob();
}

export { ApiError };

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

export const logout = () => request('/auth/logout', { method: 'POST' });

export interface CurrentUser {
  username: string;
  role_name: string;
  role_id: string;
  business_name: string;
}
export const getCurrentUser = (): Promise<CurrentUser> => request('/auth/me');

export interface MyCapabilities {
  is_admin_tier: boolean;
  can_sell: boolean;
  can_stocktake: boolean;
  can_view_reports: boolean;
  readable_modules: string[];
}
export const getMyCapabilities = (): Promise<MyCapabilities> => request('/auth/me/capabilities');

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

export const getTerms = (): Promise<{ version: string; text: string }> => request('/terms');
export const acceptTerms = (): Promise<{ version: string; accepted: boolean }> =>
  request('/terms/accept', { method: 'POST' });

export interface SecurityQuestions { question1: string | null; question2: string | null }
export const getSecurityQuestions = (biz: string, username: string): Promise<SecurityQuestions> =>
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

export const getBusinessInfo = () => request('/business');
export const listModules = () => request('/modules');
export const enableModule = (moduleId: string) => request(`/modules/${moduleId}/enable`, { method: 'POST' });
export const disableModule = (moduleId: string) => request(`/modules/${moduleId}/disable`, { method: 'POST' });
export interface AvailableModule { id: string; display_name: string; enabled: boolean }
export const listAvailableModules = (): Promise<{ modules: AvailableModule[] }> => request('/modules/available');

export interface CartItem { inventory_record_id: string; quantity: number }
export interface CheckoutRequest {
  items: CartItem[];
  payment_method?: string;
  customer?: string;
  customer_phone?: string;
  allow_oversell?: boolean;
  on_credit?: boolean;
  due_date?: string;
  discount_pct?: number;
  idempotency_key?: string;
}
export const checkout = (req: CheckoutRequest) =>
  request('/pos/checkout', { method: 'POST', body: JSON.stringify(req) });
export const getOrder = (orderId: string) => request(`/pos/orders/${orderId}`);

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

export interface RepeatCustomerRisk {
  id: string; name: string | null; phone: string | null;
  order_count: number; avg_days_between_purchases: number;
  days_since_last_purchase: number; at_risk: boolean;
}
export const getRepeatPurchaseRisk = (): Promise<{ customers: RepeatCustomerRisk[] }> =>
  request('/customers/at-risk');

export interface RefundRequest {
  sale_id: string;
  quantity: number;
  refund_amount: number;
  reason?: string;
  restock?: boolean;
}
export const processRefund = (req: RefundRequest) =>
  request('/sales/refund', { method: 'POST', body: JSON.stringify(req) });

export const repackStock = (req: {
  source_record_id: string; source_quantity: number;
  target_record_id?: string;
  new_target_name?: string;
  new_target_unit_price?: number;
  target_quantity_produced: number; notes?: string;
}) => request('/inventory/repack', { method: 'POST', body: JSON.stringify(req) });

export interface StockTakeItem {
  id: string;
  inventory_record_id: string;
  item_name: string;
  expected_qty: number;
  counted_qty: number | null;
}
export interface StockTake {
  id: string;
  status: 'in_progress' | 'closed' | 'cancelled';
  created_at: string;
  closed_at: string | null;
  items: StockTakeItem[];
}
export interface StockTakeSummary {
  id: string;
  status: 'in_progress' | 'closed' | 'cancelled';
  created_at: string;
  closed_at: string | null;
  item_count: number;
  counted_count: number;
  max_variance_pct: number | null;
  avg_variance_pct: number | null;
}
export interface StockTakeAdjustment {
  inventory_record_id: string;
  item_name: string;
  expected_qty: number;
  counted_qty: number;
  variance: number;
  variance_pct: number | 'n/a';
  write_off_cost: number;
}
export interface StockTakeCloseResult {
  stock_take_id: string;
  items_counted: number;
  items_skipped: number;
  items_skipped_deleted: number;
  total_variance_units: number;
  total_write_off_cost: number;
  adjustments: StockTakeAdjustment[];
  skipped: { inventory_record_id: string; item_name: string; expected_qty: number }[];
  skipped_deleted: { inventory_record_id: string; item_name: string; expected_qty: number; counted_qty: number }[];
}
export const initiateStockTake = (): Promise<StockTake> =>
  request('/inventory/stocktake/initiate', { method: 'POST' });
export const getOpenStockTake = (): Promise<{ open: StockTake | null }> =>
  request('/inventory/stocktake/open');
export const getStockTakeHistory = (): Promise<{ stock_takes: StockTakeSummary[] }> =>
  request('/inventory/stocktake/history');
export const recordStockTakeCount = (stockTakeId: string, itemId: string, countedQty: number) =>
  request('/inventory/stocktake/count', {
    method: 'POST',
    body: JSON.stringify({ stock_take_id: stockTakeId, item_id: itemId, counted_qty: countedQty }),
  });
export const closeStockTake = (stockTakeId: string): Promise<StockTakeCloseResult> =>
  request(`/inventory/stocktake/${stockTakeId}/close`, { method: 'POST' });
export const cancelStockTake = (stockTakeId: string): Promise<StockTake> =>
  request(`/inventory/stocktake/${stockTakeId}/cancel`, { method: 'POST' });

export interface SettleDebtSummary {
  debt_record_id: string; party_name: string; direction: string; amount: number;
  settled: true; payment_method: string; posted_to_bookkeeping_as: 'income' | 'expense' | null;
}
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
export const getDebtSummary = (): Promise<DebtSummary> => request('/debt_credit/summary');

export interface GrossProfitSummary {
  revenue_cents: number;
  cost_cents: number;
  profit_cents: number;
  margin_pct: number | null;
  sales_count: number;
  cost_bearing_sales_count: number;
  shrinkage_cents: number;
  profit_cents_after_shrinkage: number;
  margin_pct_after_shrinkage: number | null;
}
export const getGrossProfitSummary = (): Promise<GrossProfitSummary> => request('/sales/profit-summary');

export interface ItemProfit {
  item_name: string;
  revenue_cents: number;
  cost_cents: number;
  profit_cents: number;
  margin_pct: number | null;
  sales_count: number;
  cost_bearing_sales_count: number;
  shrinkage_cents: number;
  profit_cents_after_shrinkage: number;
}
export const getProfitByItem = (limit = 20): Promise<{ items: ItemProfit[] }> =>
  request(`/sales/profit-by-item?limit=${limit}`);

export interface CategoryProfit {
  category: string;
  revenue_cents: number;
  cost_cents: number;
  profit_cents: number;
  margin_pct: number | null;
  sales_count: number;
  cost_bearing_sales_count: number;
}
export const getProfitByCategory = (range?: { start: string; end: string }): Promise<{ categories: CategoryProfit[] }> =>
  request(`/sales/profit-by-category${range ? `?${new URLSearchParams(range)}` : ''}`);

export interface ItemMarginTrend {
  item_name: string;
  current_revenue_cents: number; current_cost_cents: number; current_profit_cents: number;
  current_margin_pct: number | null; current_sales_count: number; current_cost_bearing_sales_count: number;
  previous_revenue_cents: number; previous_cost_cents: number; previous_profit_cents: number;
  previous_margin_pct: number | null; previous_sales_count: number; previous_cost_bearing_sales_count: number;
  margin_pct_change_pts: number | null;
  is_losing_money: boolean;
}
export const getProfitTrend = (periodDays = 30, limit = 20): Promise<{ items: ItemMarginTrend[] }> =>
  request(`/sales/profit-trend?period_days=${periodDays}&limit=${limit}`);

export interface DebtAgingSummary {
  bucket_1_30_amount: number;
  bucket_1_30_count: number;
  bucket_31_60_amount: number;
  bucket_31_60_count: number;
  bucket_61_90_amount: number;
  bucket_61_90_count: number;
  bucket_90_plus_amount: number;
  bucket_90_plus_count: number;
  total_overdue_amount: number;
  total_overdue_count: number;
}
export const getDebtAging = (): Promise<DebtAgingSummary> => request('/debt_credit/aging');

export interface RefundRate {
  item_name: string;
  sold_quantity: number;
  refunded_quantity: number;
  refunded_amount_cents: number;
  refund_count: number;
  refund_rate_pct: number | null;
}
export const getRefundRateByItem = (limit = 20): Promise<{ items: RefundRate[] }> =>
  request(`/sales/refund-rate?limit=${limit}`);

export interface SlowMover {
  item_name: string;
  quantity: number;
  value_at_risk_cents: number;
  last_sale_at: string | null;
  days_since_last_sale: number | null;
}
export const getSlowMovers = (days = 30, limit = 20): Promise<{ items: SlowMover[] }> =>
  request(`/inventory/slow-movers?days=${days}&limit=${limit}`);

export interface StockRunway {
  item_name: string;
  quantity: number;
  avg_daily_sales: number;
  days_of_stock_left: number | null;
}
export const getStockRunway = (days = 30, limit = 15): Promise<{ items: StockRunway[] }> =>
  request(`/inventory/stock-runway?days=${days}&limit=${limit}`);

export interface UnpricedItem {
  item_name: string;
  quantity: number;
  unit_cost_cents: number;
  unit_price_cents: number;
  missing: 'cost' | 'price' | 'both';
}
export const getUnpricedItems = (limit = 50): Promise<{ items: UnpricedItem[] }> =>
  request(`/inventory/unpriced-items?limit=${limit}`);

export interface ZeroCostPurchase {
  po_number: string;
  supplier: string;
  item_name: string;
  quantity: number;
  received: boolean;
}
export const getZeroCostPurchases = (limit = 50): Promise<{ items: ZeroCostPurchase[] }> =>
  request(`/purchasing/zero-cost?limit=${limit}`);

export interface InventoryBatch {
  id: string;
  inventory_record_id: string;
  source_po_number: string | null;
  quantity_received: number;
  quantity_remaining: number;
  unit_cost: number;
  unit_price: number;
  expiry_date: string | null;
  received_at: string;
}
export interface BatchSummary {
  inventory_record_id: string;
  item_name: string;
  total_quantity: number;
  legacy_quantity: number;
  legacy_unit_cost: number;
  legacy_unit_price: number;
  batches: InventoryBatch[];
  front_of_queue_unit_cost: number;
  front_of_queue_unit_price: number;
  front_of_queue_source: 'batch' | 'legacy';
}
export const getBatches = (inventoryRecordId: string): Promise<BatchSummary> =>
  request(`/inventory/${inventoryRecordId}/batches`);

export const updateBatchPrice = (inventoryRecordId: string, batchId: string, unitPrice: number, unitCost?: number) =>
  request(`/inventory/${inventoryRecordId}/batches/${batchId}/price`, {
    method: 'POST',
    body: JSON.stringify(unitCost === undefined ? { unit_price: unitPrice } : { unit_price: unitPrice, unit_cost: unitCost }),
  });

export interface ExpiringBatch {
  batch_id: string;
  inventory_record_id: string;
  item_name: string;
  quantity_remaining: number;
  unit_cost: number;
  unit_price: number;
  expiry_date: string;
  days_to_expiry: number;
}
export const getExpiringBatches = (withinDays = 30, limit = 50): Promise<{ items: ExpiringBatch[] }> =>
  request(`/inventory/expiring-batches?within_days=${withinDays}&limit=${limit}`);

export interface ReleaseOption {
  tag: string;
  name: string;
  published_at: string;
  is_prerelease: boolean;
}
export const listReleases = (): Promise<{ items: ReleaseOption[] }> => request(`/system/releases`);
export interface RollbackCheck {
  manifest_url: string;
  target_schema_version: number;
  current_schema_version: number;
}
export const checkRollbackTarget = (tag: string): Promise<RollbackCheck> =>
  request(`/system/releases/check?tag=${encodeURIComponent(tag)}`);

export interface DayOfWeekPattern {
  day_name: string;
  avg_revenue_cents: number;
  avg_order_count: number;
  occurrences: number;
}
export const getDayOfWeekPattern = (days = 90): Promise<{ items: DayOfWeekPattern[] }> => {
  const offsetMinutes = -new Date().getTimezoneOffset();
  return request(`/sales/day-of-week?days=${days}&offset_minutes=${offsetMinutes}`);
};

export interface HourOfDayPattern {
  hour: number;
  hour_label: string;
  period: string;
  avg_revenue_cents: number;
  avg_order_count: number;
  occurrences: number;
}
export const getHourOfDayPattern = (days = 30): Promise<{ items: HourOfDayPattern[] }> => {
  const offsetMinutes = -new Date().getTimezoneOffset();
  return request(`/sales/hour-of-day?days=${days}&offset_minutes=${offsetMinutes}`);
};

export interface PeriodTrendPoint {
  label: string;
  revenue_cents: number;
  order_count: number;
  is_complete: boolean;
}
export const getMonthlyTrend = (months = 12): Promise<{ items: PeriodTrendPoint[] }> => {
  const offsetMinutes = -new Date().getTimezoneOffset();
  return request(`/sales/monthly-trend?months=${months}&offset_minutes=${offsetMinutes}`);
};

export interface SeasonalMonthPattern {
  month_name: string;
  avg_revenue_cents: number;
  years_seen: number;
}
export const getSeasonalPattern = (): Promise<{ items: SeasonalMonthPattern[] }> => {
  const offsetMinutes = -new Date().getTimezoneOffset();
  return request(`/sales/seasonal?offset_minutes=${offsetMinutes}`);
};

export interface ReportHighlights {
  most_urgent_item: { item_name: string; days_of_stock_left: number } | null;
  busiest_day: { day_name: string; avg_revenue_cents: number } | null;
}
export const getReportHighlights = (): Promise<ReportHighlights> => {
  const offsetMinutes = -new Date().getTimezoneOffset();
  return request(`/reports/highlights?offset_minutes=${offsetMinutes}`);
};

export interface BasketPair {
  item_a: string;
  item_b: string;
  order_count: number;
  combined_revenue_cents: number;
}
export const getBasketAffinity = (params: { start?: string; end?: string; limit?: number } = {}): Promise<{ pairs: BasketPair[] }> => {
  const qs = new URLSearchParams();
  if (params.start) qs.set('start', params.start);
  if (params.end) qs.set('end', params.end);
  if (params.limit) qs.set('limit', String(params.limit));
  const suffix = qs.toString();
  return request(`/sales/basket-affinity${suffix ? `?${suffix}` : ''}`);
};

export const getModuleSchema = (moduleId: string) => request(`/modules/${moduleId}/schema`);
export const listRecords = (moduleId: string, search?: string) =>
  request(`/modules/${moduleId}/records${search ? `?search=${encodeURIComponent(search)}` : ''}`);
export const lookupPosProducts = (search?: string) =>
  request(`/pos/products${search ? `?search=${encodeURIComponent(search)}` : ''}`);
export const getPosLowStock = (): Promise<{ items: { name: string; quantity: number; reorder_level: number }[] }> =>
  request('/pos/low-stock');
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
export const getBusinessPulse = (): Promise<{ business_pulse: BusinessPulse }> => request('/ai/pulse');
export const getAiContext = () => request('/ai/context');

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

export const listRoles = () => request('/roles');
export const createRole = (name: string) =>
  request('/roles', { method: 'POST', body: JSON.stringify({ name }) });
export const deleteRole = (roleId: string) =>
  request(`/roles/${roleId}`, { method: 'DELETE' });
export const setRoleAdminFlag = (roleId: string, canAdminister: boolean) =>
  request(`/roles/${roleId}/admin-flag`, { method: 'PUT', body: JSON.stringify({ can_administer: canAdminister }) });
export const setRoleReportsFlag = (roleId: string, canViewReports: boolean) =>
  request(`/roles/${roleId}/reports-flag`, { method: 'PUT', body: JSON.stringify({ can_view_reports: canViewReports }) });
export const getRolePermissions = (roleId: string) => request(`/roles/${roleId}/permissions`);
export const setRolePermissions = (roleId: string, moduleId: string, actions: string[]) =>
  request(`/roles/${roleId}/permissions`, { method: 'PUT', body: JSON.stringify({ module_id: moduleId, actions }) });

export const listUsers = () => request('/users');
export const createUser = (payload: {
  username: string; password: string; role_id: string;
  security_q1: string; security_a1: string; security_q2: string; security_a2: string;
}) => request('/users', { method: 'POST', body: JSON.stringify(payload) });
export const setUserRole = (userId: string, roleId: string) =>
  request(`/users/${userId}/role`, { method: 'PUT', body: JSON.stringify({ role_id: roleId }) });
export const deactivateUser = (userId: string) =>
  request(`/users/${userId}`, { method: 'DELETE' });

export const listUnits = () => request('/units');
export const createUnit = (name: string, abbreviation?: string) =>
  request('/units', { method: 'POST', body: JSON.stringify({ name, abbreviation }) });
export const deleteUnit = (unitId: string) => request(`/units/${unitId}`, { method: 'DELETE' });

export const listCurrencies = () => request('/currencies');
export const createCurrency = (code: string, symbol?: string, name?: string) =>
  request('/currencies', { method: 'POST', body: JSON.stringify({ code, symbol, name }) });
export const deleteCurrency = (currencyId: string) => request(`/currencies/${currencyId}`, { method: 'DELETE' });

export interface CurrencyRate { from_currency: string; to_currency: string; rate: number; fetched_at: number }
export const getCurrencyRates = (base: string): Promise<{ rates: CurrencyRate[]; stale: boolean }> =>
  request(`/currency/rates?base=${encodeURIComponent(base)}`);
export const convertCurrency = (from: string, to: string, amountCents: number): Promise<{ result: number }> =>
  request('/currency/convert', { method: 'POST', body: JSON.stringify({ from, to, amount: amountCents }) });
export const refreshCurrencyRates = (base: string) =>
  request(`/currency/refresh?base=${encodeURIComponent(base)}`, { method: 'POST' });

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

export interface NewInvoiceItem { description: string; quantity: number; unit_price: number }
export const createInvoice = (payload: {
  customer: string;
  customer_email?: string;
  customer_phone?: string;
  due_date: string;
  items: NewInvoiceItem[];
  notes?: string;
}) => request('/invoices', { method: 'POST', body: JSON.stringify(payload) });

export const markInvoiceSent = (invoiceId: string) => request(`/invoices/${invoiceId}/send`, { method: 'POST' });
export const markInvoicePaid = (invoiceId: string) => request(`/invoices/${invoiceId}/pay`, { method: 'POST' });
export const cancelInvoice = (invoiceId: string) => request(`/invoices/${invoiceId}/cancel`, { method: 'POST' });

export interface InvoiceRefundStatus { refunded_amount: number; is_refunded: boolean }
export const getInvoiceRefundStatus = (invoiceId: string): Promise<InvoiceRefundStatus> =>
  request(`/invoices/${invoiceId}/refund-status`);

export const changeBusinessType = (businessType: string): Promise<{ enabled_modules: string[] }> =>
  request('/onboarding/setup', { method: 'POST', body: JSON.stringify({ business_type: businessType }) });

export interface AuditLogEntry {
  id: string;
  user_id: string | null;
  module_id: string;
  action: string;
  record_id: string | null;
  details: unknown;
  timestamp: string;
}
export interface AuditLogFilters {
  moduleId?: string;
  recordId?: string;
  userId?: string;
  action?: string;
  from?: string;
  to?: string;
  limit?: number;
}

function auditQuery(f: AuditLogFilters): string {
  const p = new URLSearchParams();
  p.set('limit', String(f.limit ?? 200));
  if (f.moduleId) p.set('module_id', f.moduleId);
  if (f.recordId) p.set('record_id', f.recordId);
  if (f.userId) p.set('user_id', f.userId);
  if (f.action) p.set('action', f.action);
  if (f.from) p.set('from', f.from);
  if (f.to) p.set('to', f.to);
  return p.toString();
}

export const getAuditLog = (f: AuditLogFilters = {}): Promise<{ entries: AuditLogEntry[] }> =>
  request(`/audit-log?${auditQuery(f)}`);

export const exportAuditLog = async (f: AuditLogFilters = {}) => {
  const blob = await request(`/audit-log.xlsx?${auditQuery(f)}`);
  downloadBlob(blob, 'audit_log.xlsx');
};

export interface StockMovement {
  id: string;
  inventory_record_id: string;
  item_name: string;
  movement_type: string;
  movement_label: string;
  quantity_delta: number;
  unit_cost_cents: number;
  total_cost_cents: number;
  reference_id: string | null;
  user_id: string | null;
  created_at: string;
}
export interface StockMovementResult {
  movements: StockMovement[];
  total_quantity_in: number;
  total_quantity_out: number;
  net_quantity_change: number;
}
export interface StockMovementFilters {
  inventoryRecordId?: string;
  movementType?: string;
  userId?: string;
  from?: string;
  to?: string;
  limit?: number;
}

function movementQuery(f: StockMovementFilters): string {
  const p = new URLSearchParams();
  p.set('limit', String(f.limit ?? 200));
  if (f.inventoryRecordId) p.set('inventory_record_id', f.inventoryRecordId);
  if (f.movementType) p.set('movement_type', f.movementType);
  if (f.userId) p.set('user_id', f.userId);
  if (f.from) p.set('from', f.from);
  if (f.to) p.set('to', f.to);
  return p.toString();
}

export const getStockMovements = (f: StockMovementFilters = {}): Promise<StockMovementResult> =>
  request(`/inventory/movements?${movementQuery(f)}`);

export const exportStockMovements = async (f: StockMovementFilters = {}) => {
  const blob = await request(`/inventory/movements.xlsx?${movementQuery(f)}`);
  downloadBlob(blob, 'stock_movements.xlsx');
};

export const MOVEMENT_TYPES: { value: string; label: string }[] = [
  { value: 'sale', label: 'Sale' },
  { value: 'refund_restock', label: 'Refund restock' },
  { value: 'receiving', label: 'Stock received' },
  { value: 'repack_consumed', label: 'Repack (consumed)' },
  { value: 'repack_produced', label: 'Repack (produced)' },
  { value: 'stock_take_shrinkage', label: 'Stock take shrinkage' },
  { value: 'stock_take_surplus', label: 'Stock take surplus' },
];

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
