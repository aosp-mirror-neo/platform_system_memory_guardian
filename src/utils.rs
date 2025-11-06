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
use processgroup::processgroup;

// The result of searching for a process.  This contains only the pid and actual uid.
#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub uid: u32,
}

impl Process {
    pub fn new(uid: u32, pid: u32) -> Self {
        Process { uid, pid }
    }

    // Assign a set of task profiles to a process.  Return true for success.
    pub fn assign_profiles(&self, profiles: &[&str]) -> bool {
        match processgroup::set_process_profiles(self.uid, self.pid.try_into().unwrap(), profiles) {
            Ok(_) => true,
            Err(e) => {
                // Use debug formatting {:?} to print the slice of profiles
                error!("failed to set profiles {:?} on pid {}: {}", profiles, self.pid, e);
                false
            }
        }
    }
}
