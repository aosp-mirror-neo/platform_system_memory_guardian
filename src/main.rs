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
use inotify::{EventMask, WatchDescriptor, WatchMask};
use log::{error, info, warn, LevelFilter};
use nix::poll::PollTimeout;
use nix::sys::epoll::EpollEvent;
use rand::Rng;
use regex::Regex;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use thiserror::Error;

mod config;
use config::Configuration;
use config::Specification;

mod logging;

mod utils;
use utils::Process;

mod watcher;
use watcher::Watcher;

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

// The MonitoredProcess contains: its specification and its process.  The process is optional, because
// the process might not exist.  The monitor is option; it exists only if processes are being
// monitored for heap dumps.  The structure includes three flags:
//  found - the process was found and profiles were configured.
//  handled - a memory event has been seen on this process; no further monitoring
//  ignore - the process specification is invalid; do not try to use it
// The proc field may be empty while found is true if handled is true.
struct MonitoredProcess {
    spec: Specification,
    process: Option<Process>,
    monitor: Option<TargetMonitor>,
    found: bool,
    handled: bool,
    ignore: bool,
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

    let config = Configuration::load_config();
    if config.targets.is_empty() {
        warn!("pmgd found no vendor config in vendor/etc");
        nix::unistd::pause();
    }

    // Actually monitor the system.
    #[cfg(unix)]
    if let Err(e) = monitor_processes(&config) {
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
    let target_specs = &config.targets;

    // Construct the target state, one for every specification.
    let mut targets: Vec<MonitoredProcess> = Vec::new();
    for t in target_specs {
        targets.push(MonitoredProcess {
            spec: t.clone(),
            process: None,
            monitor: None,
            found: false,
            handled: false,
            ignore: false,
        });
    }

    let mut watcher = Watcher::new()?;

    // Buffer for received epoll events.
    let mut events = vec![EpollEvent::empty(); 10];

    // Update the process list once, before the loop.
    update_processes(&mut targets, &mut watcher);
    if !any_active(&targets) {
        return Ok(());
    }

    // Buffer to receive the inotify events, hoisted out of the loop.
    let mut buffer = [0u8; 4096];
    // The monitor loop.  Update processes and then wait for events.
    while let Ok(num_events) = watcher.wait(&mut events, PollTimeout::NONE) {
        if num_events > 0 {
            let _ = atrace::begin_scoped_event(AtraceTag::ActivityManager, "memory_events");
            if let Ok(events) = watcher.inotify.read_events(&mut buffer) {
                for e in events {
                    memory_event(e, &mut targets, &mut watcher);
                }
            }
        }
        update_processes(&mut targets, &mut watcher);
        if !any_active(&targets) {
            break;
        }
    }

    Ok(())
}

// Update the process information for a list of targets.  Return true if there are any active
// processes.
fn update_processes(targets: &mut Vec<MonitoredProcess>, watch: &mut Watcher) {
    atrace::trace_method!(AtraceTag::ActivityManager);

    for t in targets {
        if t.ignore {
            // The process was specified with an invalid task profile and is not properly
            // configured.
            continue;
        }

        // Check if a target we found earlier is still valid.
        if let Some(process) = &t.process {
            if !process.is_alive() {
                info!("dropping {}", t.spec.target_cmd);
                detach_process(t, watch);
            }
        }

        // This block is hit if the target was not known when the function was entered, or if
        // the previously recorded process exited.
        if !t.found {
            if let Ok(p) = Process::from(&t.spec) {
                let key = format!("{} - {}", t.spec.target_cmd, p);
                if !p.assign_profile(&t.spec) {
                    warn!("failed to configure {} - ignoring", key);
                    t.process = None;
                    t.ignore = true;
                    continue;
                }
                if watch.add_process(&p).is_err() {
                    warn!("failed to watch {} - ignoring", key);
                    t.process = None;
                    t.ignore = true;
                    continue;
                }
                info!("acquired {}", key);
                t.process = Some(p);
                t.found = true;

                if !t.handled {
                    // Attach for monitoring if the daemon has not seen any previous memory
                    // events.
                    if let Some(p) = &t.process {
                        t.monitor = attach_process(p, watch);
                    }
                    if t.monitor.is_some() {
                        info!("attached {}", key);
                    } else {
                        // The process could not be monitored.
                        warn!("not monitoring {}", key);
                    }
                } else {
                    info!("found {} already handled", key);
                }
            }
        }
    }
}

// Return true if there are any active processes left to monitor or configure.
fn any_active(targets: &Vec<MonitoredProcess>) -> bool {
    for t in targets {
        if !t.ignore {
            return true;
        }
    }
    false
}

// Given a Process, create a TargetProcess.  If heap dumps are desired, the process is also
// monitored.
fn attach_process(proc: &Process, watcher: &mut Watcher) -> Option<TargetMonitor> {
    let epath = proc.events_path();

    let start = match read_memory_high(&epath) {
        Ok(h) => h,
        Err(e) => {
            error!("failed to read {}: {}", epath, e);
            watcher.delete_process(proc);
            return None;
        }
    };

    let desc = match watcher.add_path(&epath, WatchMask::MODIFY) {
        Ok(d) => d,
        Err(_) => {
            watcher.delete_process(proc);
            return None;
        }
    };

    Some(TargetMonitor { events_path: epath, events_wd: desc, high_events: start })
}

// Detach a process that was successfully attached.
fn detach_process(target: &mut MonitoredProcess, watch: &mut Watcher) {
    atrace::trace_method!(AtraceTag::ActivityManager);
    if let Some(p) = &target.process {
        info!("detaching {} - {}", target.spec.target_cmd, p);
        if let Some(m) = &target.monitor {
            watch.remove_path(m.events_wd.clone());
        }
        watch.delete_process(p);
        target.found = false;
        target.monitor = None;
        target.process = None;
    }
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
                        handle_memory_high_event(t);
                        // Now forget proc.  It is no longer watched.
                        detach_process(t, watcher);
                    } else {
                        warn!("event on {} but no change in high", t.spec.target_cmd);
                    }
                }
            }
        }
    }
}

// Read the memory high count from the events file.
fn read_memory_high(path: &str) -> Result<u64, MyError> {
    atrace::trace_method!(AtraceTag::ActivityManager);
    let stats = fs::read_to_string(path)?;
    let high_re = Regex::new(r"high\s+(\d+)")?;
    if let Some(captures) = high_re.captures(&stats) {
        Ok(captures[1].parse()?)
    } else {
        Err(MyError::NotFound)
    }
}

// Handle a high-memory event.  A heap dump is triggered and the process is killed.  This method
// does nothing if the process has been handled once already.
fn handle_memory_high_event(monitored_process: &mut MonitoredProcess) {
    atrace::trace_method!(AtraceTag::ActivityManager);
    monitored_process.handled = true;

    if let (Some(process), Some(uid)) = (&monitored_process.process, monitored_process.spec.uid) {
        logging::log_threshold_exceeded(
            uid as i32,
            &monitored_process.spec.target_cmd,
            process.pid.try_into().unwrap(),
        );
    }

    let _ = create_perfetto_trace();
    // Note: it would be very surprising to reach this point with proc unset.
    if pmgd_flags::process_kill_enabled() {
        if let Some(process) = &monitored_process.process {
            process.kill();
        }
    }
    // Do not mark the process lost (even though it should be killed).  The lost process will be
    // detected in the main loop.
}

fn create_perfetto_trace() -> io::Result<()> {
    let trace_template = include_str!("../heap_dump.cfg");

    let random_msb: u32 = rand::thread_rng().gen();
    let trace_config = trace_template.replace("TARGET_MSB", &random_msb.to_string());

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
