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
// Standard library imports for File I/O

use anyhow::{Context, Result};
use log::{error, info};
use std::path::Path;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;

// The daemon configuration.  This is primarily used in development.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Configuration {
    pub targets: Vec<Specification>,
}

impl Configuration {
    pub fn load_config_from_path<P: AsRef<Path>>(path: P) -> Result<Configuration> {
        let path = path.as_ref();

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let config: Configuration = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse JSON content from {:?}", path))?;

        info!("Using Vendor Config. {}", content);

        Ok(config)
    }

    pub fn load_config() -> Self {
        let vendor_config_path = "/vendor/etc/config.json";
        match Self::load_config_from_path(vendor_config_path) {
            Ok(config) => config,
            Err(e) => {
                error!("failed to read {}: {}", vendor_config_path, e);
                if !pmgd_flags::memory_guardian_uses_vendor_config() {
                    Self {
                        targets: vec![Specification {
                            target_cmd: "system_server".to_string(),
                            uid: Some(1000),
                            mem_limit_profile: "SystemServerMemoryHighLimitP99".to_string(),
                            anon_limit_in_mb: None,
                            additional_task_profiles: None,
                        }],
                    }
                } else {
                    Self { targets: vec![] }
                }
            }
        }
    }
}

/**
 * Identifies a process to be monitored with an applicable memory limiting
 * profile, an applicable anonymous memory limit, and additional task profiles
 * that can be applied to the process when our daemon starts monitoring.
 */
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Specification {
    pub target_cmd: String,
    pub mem_limit_profile: String,
    pub uid: Option<u32>,
    pub anon_limit_in_mb: Option<u32>,
    pub additional_task_profiles: Option<Vec<String>>,
}

impl fmt::Display for Specification {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.target_cmd)
    }
}
