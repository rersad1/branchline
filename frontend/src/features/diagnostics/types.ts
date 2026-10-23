export interface MemoryProcess {
  pid:number;parent_pid:number;name:string;category:string;relation:string;
  resident_bytes:number|null;private_bytes:number|null;pss_bytes:number|null;swap_bytes:number|null;
  threads:number;cpu_time_ms:number|null;start_id:number|null;session_id:string|null;worktree_id:string|null;metrics_error:string|null;
}
export interface MemorySnapshot {
  platform:string;collector_pid:number;
  system:{total_bytes:number|null;available_bytes:number|null;free_bytes:number|null;cached_bytes:number|null;reclaimable_slab_bytes:number|null;unreclaimable_slab_bytes:number|null;anonymous_bytes:number|null;shmem_bytes:number|null;swap_total_bytes:number|null;swap_free_bytes:number|null};
  processes:MemoryProcess[];
  terminals:{session_id:string;project_id:string;worktree_id:string;supervisor_pid:number|null;state:string;buffer_bytes:number;buffer_capacity_bytes:number}[];
  scanned_processes:number;omitted_processes:number;pss_readable_processes:number;errors:string[];
}
export interface MemoryStatus {
  enabled:boolean;interval_seconds:number;retained_bytes:number;max_retained_bytes:number;directory:string|null;export_path:string|null;error:string|null;
  last_sample:{record_type:string;schema_version:number;captured_unix_ms:number;collection_duration_ms:number;host:MemorySnapshot|null;environments:{environment_id:string;distro:string|null;snapshot:MemorySnapshot|null;error:string|null}[];errors:string[]}|null;
}
