# Process Memory Guardian Daemon (pmgd)

## Overview

The Process Memory Guardian Daemon (pmgd) is a native Android daemon that
monitors the Control Groups (cgroups) memory metrics for specific target
processes, taking action when memory usage exceeds configured limits.

## Background

pmgd introduces a granular, per-process approach to memory enforcement. It
allows defining specific memory ceilings for targeted processes and gracefully
handling limit violations by logging statsd memory atoms prior to process termination.
This targeted execution prevents misbehaving processes from destabilizing the system.

## Execution Flow



The daemon uses `inotify` to listen for memory pressure events (specifically
high-memory activity using `memory.events`). When a monitored process triggers a
memory event, `pmgd` performs the following actions:

1.  **Anonymous memory check:** Evaluates the process's anonymous memory. If it
    exceeds the configured `anon_limit_in_mb`, `pmgd` kills the process
    immediately
2.  **Reclaim wait period:** If the anonymous memory is under the specified
    anonymous memory limit, `pmgd` waits for a system reclaim grace
    period (`reclaim_wait_time_secs`).
3.  **Memory evaluation after reclaim:** If the target process's `memory.current`
    remains greater than or equal to `memory.high` after the grace period, *or*
    anonymous memory exceeds `anon_limit_in_mb`, `pmgd` kills the
    process immediately

This evaluation cycle triggers on subsequent memory events until the process is killed or reclaim reduces its memory usage below the specified limits.

### System health features
*  **Reboot rate limiting:** To prevent boot loops or persistent crashes,
    `pmgd` tracks process kills in `/data/misc/pmgd/history.json`. The daemon
    restricts processes to a single `pmgd`-initiated kill per device reboot.

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

The following is an example configuration:

The cgroup task profile in `vendor/etc/task_profiles.json`:
```json
{
  "Attributes": [
    ...
    {
      "Name": "MemHigh",
      "Controller": "memory",
      "File": "memory.high"
    }
  ],
  "Profiles": [
    {
      "Name": "SystemServerMemoryHighLimit",
      "Actions": [
        {
          "Name": "SetAttribute",
          "Params":
          {
            "Name": "MemHigh",
            "Value": "1080M"
          }
        }
      ]
    }
  ]
}
```


The pmgd config `vendor/etc/pmgd/config.json`:

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
