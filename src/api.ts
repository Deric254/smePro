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

// ---- License ----
export const getLicenseStatus = () => request('/license/status');
export const activateLicense = () => request('/license/activate', { method: 'POST' });
export const payLicense = () => request('/license/pay', { method: 'POST' });

// ---- Modules ----
export const getBusinessInfo = () => request('/business');
export const listModules = () => request('/modules');
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

// ---- Notifications ----
export const listNotifications = () => request('/notifications');
export const sendLowStockAlert = (channel: string, recipient: string) =>
  request('/notifications/low-stock-alert', { method: 'POST', body: JSON.stringify({ channel, recipient }) });

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

// ---- Vendor license key redemption ----
export const getVendorLicenseStatus = () => request('/license/vendor/status');
export const redeemVendorKey = (key: string) =>
  request('/license/vendor/redeem', { method: 'POST', body: JSON.stringify({ key }) });
