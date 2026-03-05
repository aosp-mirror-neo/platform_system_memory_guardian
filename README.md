# Process Memory Guardian Daemon (pmgd)

## Overview

The Process Memory Guardian Daemon (pmgd) is a native Android daemon that
monitors the Control Groups (cgroups) memory metrics for specific target
processes, taking action when memory usage exceeds configured limits.

## Background

pmgd introduces a granular, per-process approach to memory enforcement. It
allows defining specific memory ceilings for targeted processes and gracefully
handling limit violations by collecting debugging information (like Perfetto
heap dumps) prior to process termination. This targeted execution prevents
misbehaving processes from destabilizing the system while providing actionable
diagnostic data.

## Execution Flow

The daemon listens for memory pressure events (specifically high-memory activity
via `memory.events`) using `inotify`. When a monitored process triggers a memory
event, `pmgd` performs the following actions:

1.  **Heap Dump Collection**: Triggers a Perfetto Java heap dump using the
    configuration defined in `/vendor/etc/pmgd/heap_dump.cfg`.

2.  **Memory Enforcement (Kill Policy)**:

    *   **High Anonymous Memory**: If the anonymous memory of the process
        exceeds the configured `anon_limit_in_mb`, the process is killed
        immediately.

    *   **Insufficient Reclaim**: If anonymous memory is within bounds, `pmgd`
        waits for a specific reclaim period (`reclaim_wait_time_secs`). If the
        process's `memory.current` remains greater than or equal to
        `memory.high` after this grace period, it is killed.

3.  **Reboot Rate Limiting**: To prevent boot loops or persistent crashes,
    `pmgd` tracks process kills in `/data/misc/pmgd/history.json`. Processes are
    restricted to one `pmgd`-initiated kill per device reboot.

## Configuration

`pmgd` configuration is vendor-driven, configured by a json file that is
required: `/vendor/etc/pmgd/config.json`. This lists the processes to trace,
their configured memory limit profile (using
[cgroup task profiles](https://source.android.com/docs/core/perf/cgroups#example-task-profiles-json-file)),
and the hard anonymous memory limit in megabytes.

### Vendor Configuration Fields

The json provided configuration will be a list of processes and their limits,
defined by the following fields:

*   `target_cmd` (String, required): The command name of the target process to
    be monitored (e.g., `"system_server"`).
*   `uid` (Integer, optional): The user ID (UID) of the process. If omitted,
    `pmgd` will apply the rule globally to any process matching `target_cmd`.
*   `reclaim_wait_time_secs` (Integer, optional): The grace period in seconds to
    wait for the memory to be reclaimed before evaluating the memory limit
    again. Defaults to `5` if omitted.
*   `mem_limit_profile` (String, required): The task profile mapped to specific
    memory limits for the target (e.g., `"SystemServerMemoryHighLimit"`).
*   `anon_limit_in_mb` (Integer, required): The ultimate anonymous memory limit
    in megabytes. If the anonymous memory utilization surpasses this value, the
    process is promptly killed.
*   `additional_task_profiles` (List of Strings, optional): A list of any
    additional task profiles that should be applied to the process when
    monitoring starts.

Example profile (`/vendor/etc/pmgd/config.json`):

```json
{
  "targets": [
    {
      "target_cmd": "system_server",
      "uid": 1000,
      "reclaim_wait_time_secs": 5,
      "mem_limit_profile": "SystemServerMemoryHighLimit",
      "anon_limit_in_mb": 500
    }
  ]
}
```
