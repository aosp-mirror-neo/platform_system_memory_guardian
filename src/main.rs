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

use inotify::{Inotify, WatchMask};
use log::error;
use log::info;
use log::warn;
use log::LevelFilter;
use regex::Regex;
use std::fs;
use std::fs::DirEntry;
use std::io;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use thiserror::Error;

mod utils;

use utils::Process;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

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
}

// Type alias for Result with MyError as the fixed error type
type MyResult<T> = std::result::Result<T, MyError>;

fn main() {
    let _init_success = logger::init(
        logger::Config::default().with_tag_on_device("pmgd").with_max_level(LevelFilter::Trace),
    );

    if pmgd_flags::memory_guardian_enabled() {
        if let Err(e) = monitor_system_server() {
            error!("{}", e)
        };
    } else {
        // This is a safe guard to ensure that pmgd runs only when AConfig flag is enabled.  If
        // the process exits, init will just restart it, so the process sleeps forever if it is
        // disabled.
        warn!("pmgd is disabled");
    }

    // Do not exit, because init will just restart the process. Wait for an external event to kill
    // this process.
    info!("waiting to be restarted");
    nix::unistd::pause();
}

// Monitor the system server process.
fn monitor_system_server() -> MyResult<()> {
    info!("pmgd started");

    let ss_uid = 1000;
    let ss_gid = 1000;
    let ss_pid = find_pid(ss_uid, ss_gid, "system_server")?;

    let profiles_to_assign = ["SystemServerMemoryHighLimitP99"];
    let guardian_process = Process::new(ss_uid, ss_pid);
    let success = guardian_process.assign_profiles(&profiles_to_assign);

    if success {
        // The path to the cgroup memory event file.
        let ss_events_path = format!("{}/memory.events", cgroup_path(ss_uid, ss_pid));
        return monitor_process(&ss_events_path);
    }
    Ok(())
}

// Return the cgroup path for a process.
fn cgroup_path(uid: u32, pid: u32) -> String {
    // See Process.java
    let first_application_uid: u32 = 10000;
    let group = if uid < first_application_uid { "system" } else { "apps" };
    format!("/sys/fs/cgroup/{}/uid_{}/pid_{}", group, uid, pid)
}

// Return the pid of process. This method walks /proc looking for the process with matching uid,
// gid, and cmdline properties.
fn find_pid(uid: u32, gid: u32, cmd: &str) -> MyResult<u32> {
    let proc_path = Path::new("/proc");
    if !proc_path.is_dir() {
        return Err(MyError::Application { msg: "failed to open /proc".into() });
    }

    let listing = fs::read_dir(proc_path)?;
    for entry in listing.flatten() {
        if let Ok(pid) = is_target_process(entry, uid, gid, cmd) {
            return Ok(pid);
        }
    }

    // At this point, the code has examined all the entries in/proc and has failed to find the
    // target process.  Return the "not found" condition.
    Err(MyError::Application { msg: "no match found".to_string() })
}

// If the /proc directory entry matches the uid, gid, and command line, return the associated pid.
fn is_target_process(entry: DirEntry, uid: u32, gid: u32, cmd: &str) -> MyResult<u32> {
    // Examine the directory entry.  The name must be valid unicode and, it must be an integer.
    // The entry must identify a directory.
    let os_name = entry.file_name();
    let name = os_name.to_str().ok_or(MyError::NotFound)?;
    let pid = name.parse()?;
    let file_type = entry.file_type().map_err(|_| MyError::NotFound)?;
    if !file_type.is_dir() {
        return Err(MyError::NotFound);
    }

    // The uid and gid of the directory entry must match the supplied values.
    #[cfg(unix)]
    if !fs::metadata(entry.path()).map(|m| uid == m.uid() && gid == m.gid()).unwrap_or(false) {
        return Err(MyError::NotFound);
    }

    let cmdline = fs::read_to_string(entry.path().join("cmdline"))?;

    // The string read from /proc/<pid>/cmdline is often padded with trailing spaces.  Until that
    // is understood, this code matches on leading text.  This is fine when the UID is a system
    // UID, since there will not be any conflicts.
    if cmdline.starts_with(cmd) {
        return Ok(pid);
    }

    Err(MyError::NotFound)
}

// Monitor memory events on a process. The process is identified by its cgroup path.  There is no
// return value.  Outside of linux, this is an empty process.
#[cfg(unix)]
fn monitor_process(events_path: &str) -> MyResult<()> {
    info!("pmgd monitoring {}", events_path);

    let mut high = read_memory_high(events_path)?;
    info!("read memory high events {}", high);

    // The intended behavior is that the first event will terminate the program.  For development,
    // events are processed in a loop.
    loop {
        let mut inotify: Inotify = Inotify::init()?;
        inotify.watches().add(events_path, WatchMask::MODIFY)?;

        let mut buffer = [0u8; 4096]; // Buffer to read events into
        let _events = inotify.read_events_blocking(&mut buffer)?;
        let new_high = read_memory_high(events_path)?;
        if high != new_high {
            info!("new high event {}", new_high);
            let _ = create_perfetto_trace();

            // Stop monitoring after a heap dump
            break;
        } else {
            info!("no change to high event");
        }
        high = new_high;

        // Prevent a notification spam by sleeping 5s.  Note that the inotify object is dropped.
        // This means that events that occur during sleeping will be ignored, which is reasonable
        // for a debug image.
        thread::sleep(Duration::from_secs(5));
    }
    Ok(())
}

// Read the memory high count from the events file.
fn read_memory_high(path: &str) -> Result<u64, MyError> {
    let stats = fs::read_to_string(path)?;
    let high_re = Regex::new(r"high\s+(\d+)")?;
    if let Some(captures) = high_re.captures(&stats) {
        Ok(captures[1].parse()?)
    } else {
        Err(MyError::NotFound)
    }
}

fn create_perfetto_trace() -> io::Result<()> {
    let trace_config = include_str!("../heap_dump.cfg");

    let mut child = Command::new("/system/bin/perfetto")
        .args(["-c", "-", "--txt", "--upload"])
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

#[cfg(not(unix))]
fn monitor_process(_cgroup_path: &str) -> MyResult<()> {
    error!("pmgd not supported outside unix");
    Err(MyError::Application("not supported".into()))
}
