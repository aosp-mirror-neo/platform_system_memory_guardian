// Copyright 2027, The Android Open Source Project
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
//! This module contains features that save/load pmgd events to disk

use anyhow::{Context, Result};
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// Persisted across process restarts and system reboots.
#[derive(Serialize, Deserialize, Debug)]
pub struct MemcgKillHistory {
    pub last_kill_time: u64,
    pub last_process_kill_time: HashMap<String, u64>,
}

impl MemcgKillHistory {
    const HISTORY_PATH: &str = "/data/misc/pmgd/history.json";

    pub fn new() -> Self {
        Self { last_kill_time: 0, last_process_kill_time: HashMap::new() }
    }

    // Loads the history from disk. Returns a default history if file doesn't exist.
    pub fn load() -> Result<Self> {
        let path = Path::new(Self::HISTORY_PATH);

        if !path.exists() {
            return Ok(Self::default());
        }

        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read history file at {:?}", path))?;

        let history =
            serde_json::from_str(&data).with_context(|| "Failed to deserialize kill history")?;

        Ok(history)
    }

    pub fn save(&self) -> Result<()> {
        let path = Path::new(Self::HISTORY_PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string(self)?;
        fs::write(path, data)?;

        Ok(())
    }

    pub fn was_killed_since_reboot(&mut self, process_name: &str) -> bool {
        self.last_process_kill_time.contains_key(process_name)
    }

    pub fn record_kill_event(&mut self, process_name: &str) {
        let time_now = Self::now_in_secs();

        self.last_kill_time = time_now;
        self.last_process_kill_time.insert(process_name.to_string(), self.last_kill_time);
    }

    fn now_in_secs() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).expect("Invalid Time").as_secs()
    }
}

impl Default for MemcgKillHistory {
    fn default() -> Self {
        Self::new()
    }
}

// Global kill history singleton
static KILL_HISTORY: OnceLock<Mutex<MemcgKillHistory>> = OnceLock::new();

pub fn get_kill_history() -> &'static Mutex<MemcgKillHistory> {
    KILL_HISTORY.get_or_init(|| {
        let initial_history = MemcgKillHistory::load().unwrap_or_else(|e| {
            info!("pmgd: {}", e);
            MemcgKillHistory::new()
        });
        Mutex::new(initial_history)
    })
}
