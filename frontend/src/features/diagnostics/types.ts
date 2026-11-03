export interface MemoryProcess {
  pid:number;parent_pid:number;name:string;category:string;relation:string;
  resident_bytes:number|null;private_bytes:number|null;pss_bytes:number|null;swap_bytes:number|null;
  threads:number;cpu_time_ms:number|null;start_id:number|null;session_id:string|null;worktree_id:string|null;metrics_error:string|null;
  lifetime_id?:string|null;role?:string;classification?:string;container?:boolean|null;
  resident_delta_bytes?:number|null;pss_delta_bytes?:number|null;cpu_delta_ms?:number|null;cpu_percent?:number|null;peak_resident_bytes?:number|null;peak_pss_bytes?:number|null;
}
export interface MemoryTotals {processes:number;resident_bytes:number;pss_bytes:number;swap_bytes:number;threads:number;cpu_time_ms:number;missing_rss:number;missing_pss:number;missing_swap:number;missing_cpu:number;missing_start:number;accounted_bytes:number;missing_accounting:number}
export interface MemoryGroup {scope:string;worktree_id:string|null;totals:MemoryTotals;categories:{category:string;totals:MemoryTotals}[];peak_accounted_bytes:number;accounted_delta_bytes:number|null}
export interface BudgetSetting {environment_id:string;worktree_id:string;limit_bytes:number|null}
export interface BudgetStatus extends BudgetSetting {state:string;accounted_bytes:number|null;excess_bytes:number|null;peak_accounted_bytes:number;complete:boolean;missing_pss:number;missing_accounting:number;fallback_rss:boolean;warning_active:boolean}
export interface MemoryEvent {kind:string;environment_id:string;worktree_id:string|null;previous_worktree_id:string|null;lifetime_id:string|null;reason:string}
export interface MemorySnapshot {
  platform:string;collector_pid:number;
  system:{total_bytes:number|null;available_bytes:number|null;free_bytes:number|null;cached_bytes:number|null;reclaimable_slab_bytes:number|null;unreclaimable_slab_bytes:number|null;anonymous_bytes:number|null;shmem_bytes:number|null;swap_total_bytes:number|null;swap_free_bytes:number|null;pressure_some?:{avg10:number;avg60:number;avg300:number;total_us:number}|null;pressure_full?:{avg10:number;avg60:number;avg300:number;total_us:number}|null};
  processes:MemoryProcess[];
  terminals:{session_id:string;project_id:string;worktree_id:string;supervisor_pid:number|null;state:string;buffer_bytes:number;buffer_capacity_bytes:number}[];
  scanned_processes:number;omitted_processes:number;pss_readable_processes:number;errors:string[];
  source_id?:string;groups?:MemoryGroup[];coverage?:{discovered_processes:number;unreadable_processes:number;excluded_processes:number;detailed_processes:number;inventory_complete:boolean};
}
export interface MemoryStatus {
  enabled:boolean;interval_seconds:number;retained_bytes:number;max_retained_bytes:number;directory:string|null;export_path:string|null;error:string|null;
  budget_settings?:BudgetSetting[];budgets?:BudgetStatus[];
  last_sample:{record_type:string;schema_version:number;captured_unix_ms:number;collection_duration_ms:number;host:MemorySnapshot|null;environments:{environment_id:string;distro:string|null;snapshot:MemorySnapshot|null;error:string|null}[];errors:string[];host_environment_id?:string;session_id?:string;snapshot_bytes?:number;events?:MemoryEvent[];omitted_events?:number;budget_settings?:BudgetSetting[];budgets?:BudgetStatus[]}|null;
}
