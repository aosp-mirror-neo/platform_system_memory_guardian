// Copyright 2025, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! pmgd (process memory guardian) is a native daemon monitoring memcg memory for a process.

use atrace::AtraceTag;
use inotify::{EventMask, WatchDescriptor};
use log::{error, info, warn, LevelFilter};
use nix::poll::PollTimeout;
use nix::sys::epoll::EpollEvent;
use rand::Rng;
use std::error::Error;
use std::fmt;
use std::io;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use thiserror::Error;

mod config;
use config::Configuration;
use config::Specification;

mod os_api;
use os_api::ProcessGroupApi;
use os_api::ProcessGroupApiImpl;
mod on_disk_utils;
use crate::on_disk_utils::get_kill_history;

#[cfg(test)]
mod integration_tests;

mod monitored_processes;
use monitored_processes::MonitoredProcess;
use monitored_processes::MonitoredProcesses;

mod memcg_proc_memory_util;
use memcg_proc_memory_util::{
    read_cgroup_memory_current_kb, read_cgroup_memory_high_kb, read_cgroup_memory_stat,
    read_cgroup_memory_swap_kb, read_memory_high,
};

mod logging;
use logging::{log_anon_memory_breach, log_threshold_exceeded, log_total_memory_breach};

mod utils;
use utils::Process;

mod watcher;
use watcher::Watcher;

const HOLD_BACK_AFTER_SUCCESSFUL_RECLAIM_IN_SECONDS: u32 = 300;

// Local error class that collects the different kinds of errors that might be reported.
#[derive(Error, Debug)]
enum MyError {
    #[error("error: not found")]
    NotFound,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    ParseError(#[from] std::num::ParseIntError),
    #[error(transparent)]
    RegexError(#[from] regex::Error),
    #[error("error: {msg}")]
    Application { msg: String },
    #[error("errno")]
    Errno(#[from] nix::errno::Errno),
}

#[derive(PartialEq, Debug)]
enum HighMemoryEventResult {
    KilledDueToHighAnonMemory,
    KilledDueToInsufficientReclaim,
    NoOpDueToInvalidConfigParsing,
    NoOpDueToFailedCgroupRead,
    NoOpDueToDisabledKill, // TODO remove once process_kill_enabled is removed
    ReclaimSuccessful,
}

// Type alias for Result with MyError as the fixed error type
type MyResult<T> = std::result::Result<T, MyError>;

// The information required to monitor a process and respond to memory events.
struct TargetMonitor {
    // The path to the cgroups memory.events entry for the process.
    events_path: String,
    // The watch descriptor for the memory.events entry.
    events_wd: WatchDescriptor,
    // The last-seen value of the high memory events.
    high_events: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let _init_success = logger::init(
        logger::Config::default().with_tag_on_device("pmgd").with_max_level(LevelFilter::Trace),
    );

    if !pmgd_flags::memory_guardian_enabled() {
        // This is a safe guard to ensure that pmgd runs only when AConfig flag is enabled.  If
        // the process exits, init will just restart it, so the process sleeps forever if it is
        // disabled.
        warn!("pmgd is disabled");
        nix::unistd::pause();
    }

    let mut vendor_config = Configuration::load_config();
    filter_valid_config_targets(&mut vendor_config);
    if vendor_config.targets.is_empty() {
        warn!("The list of config targets is empty");
        nix::unistd::pause();
    }

    // Actually monitor the system.
    #[cfg(unix)]
    if let Err(e) = monitor_processes(&vendor_config) {
        error!("{}", e)
    }

    // Do not exit, because init will just restart the process. Wait for an external event to kill
    // this process.
    info!("waiting to be restarted");
    nix::unistd::pause();

    // The universally-recognized "everything is okay" status
    Ok(())
}

// Monitor the process list.
fn monitor_processes(config: &Configuration) -> MyResult<()> {
    let mut monitored_processes = MonitoredProcesses::from(config);
    let mut watcher = Watcher::new()?;

    // Buffer for received epoll events.
    let mut events = vec![EpollEvent::empty(); 10];

    // Update the process list once, before the loop.
    monitored_processes.update_processes(&mut watcher);
    if !monitored_processes.any_active() {
        return Ok(());
    }

    // Buffer to receive the inotify events, hoisted out of the loop.
    let mut buffer = [0u8; 4096];
    // The monitor loop.  Update processes and then wait for events.
    while let Ok(num_events) = watcher.wait(&mut events, PollTimeout::NONE) {
        if num_events > 0 {
            let processes_to_update = monitored_processes.get_processes_to_update();
            let _ = atrace::begin_scoped_event(AtraceTag::ActivityManager, "memory_events");
            if let Ok(events) = watcher.inotify.read_events(&mut buffer) {
                for e in events {
                    memory_event(e, processes_to_update, &mut watcher);
                }
            }
        }
        monitored_processes.update_processes(&mut watcher);

        if !monitored_processes.any_active() {
            break;
        }
    }

    Ok(())
}

// Process a single inotify event.
fn memory_event(
    e: inotify::Event<&std::ffi::OsStr>,
    targets: &mut Vec<MonitoredProcess>,
    watcher: &mut Watcher,
) {
    atrace::trace_method!(AtraceTag::ActivityManager);
    // High-memory events are trigged by a MODIFY event.  Other events are likely process
    // start/stop events.  If this is not a high-memory event, just return.
    if !e.mask.contains(EventMask::MODIFY) {
        return;
    }

    // TODO: Abstract processes.find_process_with_fired_event(fired)
    let fired = e.wd;
    for t in &mut *targets {
        if let Some(proc) = &mut t.monitor {
            if proc.events_wd == fired {
                info!("memory event on {}", t.spec.target_cmd);
                if let Ok(new_high) = read_memory_high(&proc.events_path) {
                    info!(
                        "memory event on {} high is={} was={}",
                        t.spec.target_cmd, new_high, proc.high_events
                    );
                    if new_high > proc.high_events {
                        // Ensure high events are updated
                        proc.high_events = new_high;
                        match handle_memory_high_event::<ProcessGroupApiImpl>(t) {
                            res if res != HighMemoryEventResult::ReclaimSuccessful => {
                                t.detach_process(watcher);
                            }
                            _ => {
                                info!(
                                    "Not Detaching, throttling for {} seconds",
                                    HOLD_BACK_AFTER_SUCCESSFUL_RECLAIM_IN_SECONDS
                                );
                                thread::sleep(Duration::from_secs(
                                    HOLD_BACK_AFTER_SUCCESSFUL_RECLAIM_IN_SECONDS.into(),
                                ));
                            }
                        }
                    } else {
                        warn!("event on {} but no change in high", t.spec.target_cmd);
                    }
                }
            }
        }
    }
}

// Handle a high-memory event.  A heap dump is triggered and the process is killed.  This method
// does nothing if the process has been handled once already.
fn handle_memory_high_event<P: ProcessGroupApi>(
    monitored_process: &mut MonitoredProcess,
) -> HighMemoryEventResult {
    atrace::trace_method!(AtraceTag::ActivityManager);
    monitored_process.mark_as_handled();

    if let Some(process) = &monitored_process.process {
        let uid = process.uid;
        let target_pid = process.pid;
        let mem_stats = read_cgroup_memory_stat(uid, target_pid);
        let swap_kb = read_cgroup_memory_swap_kb(uid, target_pid);
        let target_cmd = monitored_process.get_target_cmd();

        log_threshold_exceeded(uid as i32, target_cmd, &mem_stats, swap_kb);

        // LINT.IfChange
        if !pmgd_flags::process_kill_enabled() {
            // LINT.ThenChange(HighMemoryEventResult)
            return HighMemoryEventResult::NoOpDueToDisabledKill;
        }

        let Some(anon_limit_in_mb) = monitored_process.get_profile_anon_limit() else {
            return HighMemoryEventResult::NoOpDueToInvalidConfigParsing;
        };

        let anon_memory_in_mb = mem_stats.anon_kilobytes / 1024;
        warn!("pid_{} anon_memory {}/{}mb ", target_pid, anon_memory_in_mb, anon_limit_in_mb);
        if anon_memory_in_mb > anon_limit_in_mb {
            warn!(
                "killing pid_{} anon_memory {}mb > limit {}mb",
                target_pid, anon_memory_in_mb, anon_limit_in_mb
            );

            let _ = create_perfetto_trace(target_cmd);
            log_anon_memory_breach(uid as i32, target_cmd, &mem_stats, swap_kb);
            process.kill::<P>();
            record_kill_event(target_cmd);
            return HighMemoryEventResult::KilledDueToHighAnonMemory;
        } else {
            let reclaim_time_secs = monitored_process.get_spec_reclaim_wait_time_secs();
            info!("Waiting for memory reclaim for {} seconds", reclaim_time_secs);
            thread::sleep(Duration::from_secs(reclaim_time_secs.into()));

            let mem_high_opt = read_cgroup_memory_high_kb(uid, target_pid);
            let mem_curr_opt = read_cgroup_memory_current_kb(uid, target_pid);

            if mem_high_opt.is_none() || mem_curr_opt.is_none() {
                error!("Failed to read cgroup memory values for pid_{}", target_pid);
                return HighMemoryEventResult::NoOpDueToFailedCgroupRead;
            }
            let mem_high = mem_high_opt.unwrap();
            let mem_curr = mem_curr_opt.unwrap();

            if mem_curr >= mem_high {
                warn!("checking if mem_curr {}mb >= mem_high {}mb", mem_curr, mem_high);

                let _ = create_perfetto_trace(target_cmd);
                log_anon_memory_breach(uid as i32, target_cmd, &mem_stats, swap_kb);
                process.kill::<P>();
                record_kill_event(target_cmd);
                return HighMemoryEventResult::KilledDueToHighAnonMemory;
            } else {
                info!("Waiting for memory reclaim for {} seconds", reclaim_time_secs);
                thread::sleep(Duration::from_secs(reclaim_time_secs.into()));

                let mem_high_opt = read_cgroup_memory_high_kb(uid, target_pid);
                let mem_curr_opt = read_cgroup_memory_current_kb(uid, target_pid);

                if mem_high_opt.is_none() || mem_curr_opt.is_none() {
                    error!("Failed to read cgroup memory values for pid_{}", target_pid);
                    return HighMemoryEventResult::NoOpDueToFailedCgroupRead;
                }
                let mem_high = mem_high_opt.unwrap();
                let mem_curr = mem_curr_opt.unwrap();

                info!("checking if mem_curr {}mb > mem_high {}mb", mem_curr, mem_high);
                if mem_curr >= mem_high {
                    warn!(
                        "killing pid_{} mem_curr {}mb >= mem_high {}mb",
                        target_pid, mem_curr, mem_high
                    );
                    let mem_stats = read_cgroup_memory_stat(uid, target_pid);
                    let swap_kb = read_cgroup_memory_swap_kb(uid, target_pid);

                    let _ = create_perfetto_trace(target_cmd);
                    log_total_memory_breach(uid as i32, target_cmd, &mem_stats, swap_kb);
                    process.kill::<P>();
                    record_kill_event(target_cmd);
                    return HighMemoryEventResult::KilledDueToInsufficientReclaim;
                }
            }
        }
    } else {
        return HighMemoryEventResult::NoOpDueToInvalidConfigParsing;
    }
    HighMemoryEventResult::ReclaimSuccessful
}

fn create_perfetto_trace(target_cmd: &str) -> io::Result<()> {
    let trace_template = include_str!("../heap_dump.cfg");

    let random_msb: u32 = rand::thread_rng().gen();
    let trace_config = trace_template
        .replace("TARGET_MSB", &random_msb.to_string())
        .replace("TARGET_CMDLINE", target_cmd);

    let args = vec!["-c", "-", "--txt", "--upload"];

    let mut child = Command::new("/system/bin/perfetto")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write(trace_config.as_bytes()) {
            info!("Failed to write to stdin: {}", e);
        }
        drop(stdin);
    }

    info!("Waiting for perfetto to finish and collecting output...");
    let output = child.wait_with_output()?;

    if output.status.success() {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        info!("Perfetto command succeeded!");
        info!("--- Perfetto stdout ---\n{}", stdout_str);
    } else {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        error!("Perfetto command failed with status: {}", output.status);
        error!("--- Perfetto stderr ---\n{}", stderr_str);
    }
    Ok(())
}

impl fmt::Display for MonitoredProcess {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(p) = &self.process {
            p.fmt(f)
        } else {
            self.spec.fmt(f)
        }
    }
}

// remove any targets that have been killed since a device reboot
fn filter_valid_config_targets(config: &mut Configuration) {
    let mut history = get_kill_history().lock().unwrap();

    config.targets.retain(|target_process| {
        let was_killed_since_reboot = history.was_killed_since_reboot(&target_process.target_cmd);

        if was_killed_since_reboot {
            info!("Target {} was killed since reboot, filtering out.", target_process.target_cmd);
        }
        !was_killed_since_reboot
    });
}

fn record_kill_event(process_name: &str) {
    let mut history = get_kill_history().lock().unwrap();
    history.record_kill_event(process_name);
    if let Err(e) = history.save() {
        error!("Failed to persist kill history: {}", e);
    }
}
