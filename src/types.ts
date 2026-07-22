export type FieldType = 'text' | 'integer' | 'real' | 'date' | 'boolean' | 'unit' | 'currency';

export interface FieldDef {
  name: string;
  type: FieldType;
  required?: boolean;
  unique?: boolean;
  default?: unknown;
}

export interface ModuleSchema {
  id: string;
  display_name: string;
  fields: FieldDef[];
  actions: string[];
  /** What the CURRENTLY LOGGED IN user can actually do on this module —
   * a subset of `actions`, computed server-side from their role. Use
   * this to decide which buttons to show, not `actions` — that field
   * is the module's theoretical capability list, the same for every
   * user regardless of role. */
  my_permissions: string[];
}

export interface ModuleListItem {
  id: string;
  display_name: string;
  enabled: boolean;
}

export type LicenseStatus =
  | { status: 'active' }
  | { status: 'inactive' }
  | { status: 'grace'; days_left: number }
  | { status: 'locked'; days_overdue: number };

export type Record_ = { id: string; [key: string]: unknown };

export interface Role {
  id: string;
  name: string;
  is_system: boolean;
  can_administer: boolean;
}

export interface UserAccount {
  id: string;
  username: string;
  role: string;
  active: boolean;
  created_at: string;
}

export interface Unit {
  id: string;
  name: string;
  abbreviation: string | null;
}

export interface Currency {
  id: string;
  code: string;
  symbol: string | null;
  name: string | null;
}
