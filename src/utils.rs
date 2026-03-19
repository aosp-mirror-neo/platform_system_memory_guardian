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
//! This module contains system-specific utilities.

use log::error;
use nix::libc::c_int;
use std::fmt;
use std::fs;
use std::os::fd::{FromRawFd, OwnedFd};

#[cfg(not(test))]
use crate::os_api::FsApiImpl as FsApi;
#[cfg(test)]
use crate::os_api::MockFsApi as FsApi;

use crate::os_api::FsApi as _;
use crate::os_api::ProcessGroupApi;

use super::config::Specification;
use super::MyError;
use super::MyResult;

#[cfg(unix)]
extern "C" {
    fn pidfd_open(pid: c_int, flags: c_int) -> c_int;
}

// The result of searching for a process.  This contains only the pid and actual uid.
#[derive(Debug)]
pub struct Process {
    pub pid: u32,
    pub uid: u32,

    // The path to the /proc entry for this process
    cmd_path: String,

    // A pidfd
    pub pfd: Option<OwnedFd>,
}

impl Process {
    // Return the path to the memory.events file in the cgroup.
    pub fn events_path(&self) -> String {
        format!("{}/memory.events", self.cgroup_path())
    }

    // Return the cgroup path for a process.
    fn cgroup_path(&self) -> String {
        // See Process.java
        let first_application_uid: u32 = 10000;
        let group = if self.uid < first_application_uid { "system" } else { "apps" };
        format!("/sys/fs/cgroup/{}/uid_{}/pid_{}", group, self.uid, self.pid)
    }

    // Return true if the process is alive.
    pub fn is_alive(&self) -> bool {
        fs::metadata(&self.cmd_path).is_ok()
    }

    // Kill a process group.  The command parameter is used only for error logging.
    pub fn kill<P: ProcessGroupApi>(&self) -> bool {
        let kill_signal: c_int = 9;
        let pid: i32 = self.pid.try_into().unwrap();
        match P::kill_process_group(self.uid, pid, kill_signal) {
            Ok(_) => true,
            Err(e) => {
                // kill_process_group seems to successfully kill the target process but it then
                // fails to remove the associated cgroup entries and returns an error.
                error!("failed to kill process {}: {}", pid, e);
                false
            }
        }
    }

    // Assign the target spec's mem limit profile.  Return true for success.
    pub fn assign_profile<P: ProcessGroupApi>(&self, spec: &Specification) -> bool {
        let Some(profile_str) = spec.profile_info.get_mem_limit_profile() else {
            return false;
        };

        let pid: i32 = self.pid.try_into().unwrap();
        let plist = vec![profile_str.to_string()];
        match P::set_process_profiles(self.uid, pid, plist) {
            Ok(_) => true,
            Err(e) => {
                error!("failed to set profiles on {} {}: {}", spec.target_cmd, pid, e);
                false
            }
        }
    }

    // Return the uid and pid of a process. This method walks /proc looking for the process with
    // the matching cmdline.  If the uid is known, it may be specified, which tightens up the
    // security of the search.  Command line matching ignores trailing white space.
    pub fn from(spec: &Specification) -> MyResult<Self> {
        let names = FsApi::list_dir("/proc")?;
        for name in names {
            if let Ok(pid) = Self::is_process(&name, spec) {
                return Ok(pid);
            }
        }

        // At this point, the code has examined all the entries in/proc and has failed to find the
        // target process.  Return the "not found" condition.
        Err(MyError::Application { msg: "no match found".to_string() })
    }

    // If the /proc directory entry matches the uid, and command line, return the pid and uid.
    fn is_process(name: &str, target_spec: &Specification) -> MyResult<Self> {
        let process_pid: u32 = name.parse().map_err(|_| MyError::NotFound)?;

        // Capture the uid.
        let path = format!("/proc/{}", name);
        let process_uid = FsApi::get_uid(&path).map_err(|_| MyError::NotFound)?;

        // If there is a target_spec uid, match it now.
        if let Some(want_uid) = target_spec.uid {
            if want_uid != process_uid {
                return Err(MyError::NotFound);
            }
        }

        // Compare the command line.
        let cpath = format!("/proc/{}/cmdline", process_pid);
        let cmdline = FsApi::read_to_string(&cpath)?;
        if Self::trim_nulls(&cmdline) == target_spec.target_cmd {
            let pfd = Self::pid_fd(process_pid).ok();
            Ok(Process { pid: process_pid, uid: process_uid, cmd_path: cpath, pfd })
        } else {
            Err(MyError::NotFound)
        }
    }

    // The OS sometimes supplies fixed-width strings that are padded with nulls.  Take an input
    // string and trim it to the first trailing null (if any).
    fn trim_nulls(s: &str) -> &str {
        if let Some(i) = s.find('\0') {
            &s[0..i]
        } else {
            s
        }
    }

    // Return a pidfd for a process.
    fn pid_fd(pid: u32) -> MyResult<OwnedFd> {
        // SAFETY: This creates a file descriptor that may be monitored for process exit.  The
        // descriptor cannot be read but it can be passed to epoll().  pidfd_open() does not
        // place any requirements on its arguments.
        let fd = unsafe { pidfd_open(pid as i32, 0) };
        if fd >= 0 {
            // SAFETY: This fd is known a priori to be owned because it was created above.
            unsafe { Ok(OwnedFd::from_raw_fd(fd)) }
        } else {
            let emsg = format!("pidfd_open failed: errno={}", std::io::Error::last_os_error());
            Err(MyError::Application { msg: emsg })
        }
    }
}

impl fmt::Display for Process {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Use the write! macro to write into the formatter f.
        // write! returns a std::fmt::Result, so we can use ? to propagate errors.
        write!(f, "{}/{}", self.pid, self.uid)
    }
}
