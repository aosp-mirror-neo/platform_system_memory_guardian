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
//! This module contains the utilities to read MemcgV2 memory values for a process

use log::error;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;

pub struct CgroupMemStat {
    pub anon_kilobytes: u32,
    pub file_kilobytes: u32,
}

fn read_cgroup_base_path(uid: u32, pid: u32) -> PathBuf {
    // TODO(b/467035873) Support Non-System processes
    let cgroup_base = format!("/sys/fs/cgroup/system/uid_{}/pid_{}/", uid, pid);
    PathBuf::from(&cgroup_base)
}

pub fn read_cgroup_memory_stat_path(uid: u32, pid: u32) -> PathBuf {
    let mut base_path_buf = read_cgroup_base_path(uid, pid);
    base_path_buf.push("memory.stat");
    base_path_buf
}

fn read_cgroup_file_as_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok().and_then(|s| s.trim().parse::<u64>().ok())
}

pub fn read_cgroup_memory_swap(uid: u32, pid: u32) -> u32 {
    let mut path = read_cgroup_base_path(uid, pid);
    path.push("memory.swap.current");

    match read_cgroup_file_as_u64(&path) {
        Some(bytes) => (bytes / 1024) as u32,
        None => {
            error!("Failed to read swap path");
            0
        }
    }
}

pub fn read_cgroup_memory_stat(uid: u32, pid: u32) -> CgroupMemStat {
    let path = read_cgroup_memory_stat_path(uid, pid);

    let mut stats = CgroupMemStat { anon_kilobytes: 0, file_kilobytes: 0 };

    if let Ok(file) = File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let parts: Vec<&str> = line.split_whitespace().collect();

            if parts.len() >= 2 {
                let key = parts[0];
                let value = parts[1].parse::<u64>().unwrap_or(0);
                match key {
                    "anon" => stats.anon_kilobytes = (value / 1024) as u32,
                    "file" => stats.file_kilobytes = (value / 1024) as u32,
                    _ => {}
                }
            }
        }
    }
    stats
}
