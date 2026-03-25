// Copyright 2026, The Android Open Source Project
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

use super::read_memory_high;
use super::Configuration;
use super::Process;
use super::Specification;
use super::TargetMonitor;
use super::Watcher;
use inotify::WatchMask;
use log::{error, info, warn};

pub struct MonitoredProcesses {
    pub processes: Vec<MonitoredProcess>,
}

impl MonitoredProcesses {
    pub fn from(config: &Configuration) -> MonitoredProcesses {
        let target_specs = &config.targets;
        let processes = target_specs
            .iter()
            .map(|spec| MonitoredProcess {
                spec: spec.clone(),
                process: None,
                monitor: None,
                found: false,
                handled: false,
                ignore: false,
            })
            .collect();

        MonitoredProcesses { processes }
    }

    // Return true if there are any active processes left to monitor or configure.
    pub fn any_active(&self) -> bool {
        for process in &self.processes {
            if !process.ignore {
                return true;
            }
        }
        false
    }

    pub fn get_processes_to_update(&mut self) -> &mut Vec<MonitoredProcess> {
        &mut self.processes
    }

    pub fn update_processes(&mut self, watch: &mut Watcher) {
        for t in &mut self.processes {
            if t.ignore {
                continue;
            }

            // Check if a target we found earlier is still valid.
            if let Some(process) = &t.process {
                if !process.is_alive() {
                    info!("dropping {}", t.spec.target_cmd);
                    t.detach_process(watch);
                }
            }

            // This block is hit if the target was not known when the function was entered, or if
            // the previously recorded process exited.
            if !t.found {
                if let Ok(p) = Process::from(&t.spec) {
                    let key = format!("{} - {}", t.spec.target_cmd, p);

                    if !p.assign_profile::<crate::os_api::ProcessGroupApiImpl>(&t.spec) {
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
                        t.attach_process(watch);
                        info!("attach_process ");
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
}

// The MonitoredProcess contains: its specification and its process.  The process is optional, because
// the process might not exist.  The monitor is option; it exists only if processes are being
// monitored for heap dumps.  The structure includes three flags:
//  found - the process was found and profiles were configured.
//  handled - a memory event has been seen on this process; no further monitoring
//  ignore - the process specification is invalid; do not try to use it
// The proc field may be empty while found is true if handled is true.
pub struct MonitoredProcess {
    pub spec: Specification,
    pub process: Option<Process>,
    pub monitor: Option<TargetMonitor>,
    pub found: bool,
    pub handled: bool,
    pub ignore: bool,
}

impl MonitoredProcess {
    pub fn attach_process(&mut self, watcher: &mut Watcher) {
        let proc = match &self.process {
            Some(p) => p,
            None => return,
        };
        let epath = proc.events_path();

        let start = match read_memory_high(&epath) {
            Ok(h) => h,
            Err(e) => {
                error!("failed to read {}: {}", epath, e);
                watcher.delete_process(proc);
                return;
            }
        };

        let desc = match watcher.add_path(&epath, WatchMask::MODIFY) {
            Ok(d) => d,
            Err(_) => {
                watcher.delete_process(proc);
                return;
            }
        };

        self.monitor =
            Some(TargetMonitor { events_path: epath, events_wd: desc, high_events: start });
    }
    pub fn detach_process(&mut self, watch: &mut Watcher) {
        if let Some(p) = &self.process {
            info!("detaching {} - {}", self.spec.target_cmd, p);
            if let Some(m) = &self.monitor {
                watch.remove_path(m.events_wd.clone());
            }
            watch.delete_process(p);
            self.found = false;
            self.monitor = None;
            self.process = None;
        }
    }
    pub fn get_spec_reclaim_wait_time_secs(&self) -> u32 {
        self.spec.reclaim_wait_time_secs
    }
    pub fn get_profile_anon_limit(&self) -> Option<u32> {
        self.spec.profile_info.get_anon_limit_mb()
    }
    pub fn get_target_cmd(&self) -> &String {
        &self.spec.target_cmd
    }
    pub fn mark_as_handled(&mut self) {
        self.handled = true
    }
}
