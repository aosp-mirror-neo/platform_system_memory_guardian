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
//! This module contains the configuration options for the daemon.

use std::fmt;

// The daemon configuration.  This is primarily used in development.
pub struct Configuration {
    pub targets: Vec<Specification>,
}

impl Configuration {
    // A base configuration when an external configuration file is not present.
    pub fn new() -> Self {
        Configuration {
            targets: vec![Specification {
                cmd: "system_server".to_string(),
                uid: Some(1000),
                profile: vec!["SystemServerMemoryHighLimitP99".to_string()],
            }],
        }
    }
}

// A target specification: this identifies a process to be monitored and a profile to be assigned
// to that process.  The process is identified by its command line and an optional UID.  A UID
// should always be specified if it is known.
#[derive(Clone)]
pub struct Specification {
    pub cmd: String,
    pub uid: Option<u32>,
    pub profile: Vec<String>,
}

impl fmt::Display for Specification {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.cmd)
    }
}
