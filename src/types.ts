export type FieldType = 'text' | 'integer' | 'real' | 'money' | 'date' | 'boolean' | 'unit' | 'currency';

export interface FieldDef {
  name: string;
  type: FieldType;
  required?: boolean;
  unique?: boolean;
  default?: unknown;
}

export interface DashboardMetric {
  measure?: string;
  aggregation: 'sum' | 'avg' | 'count';
  label: string;
}

export interface ModuleSchema {
  id: string;
  display_name: string;
  fields: FieldDef[];
  actions: string[];
  my_permissions: string[];
  dashboard_metric?: DashboardMetric | null;
}

export interface ModuleListItem {
  id: string;
  display_name: string;
  enabled: boolean;
}

export type Record_ = { id: string; [key: string]: unknown };

export interface Role {
  id: string;
  name: string;
  is_system: boolean;
  can_administer: boolean;
  can_view_reports: boolean;
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
