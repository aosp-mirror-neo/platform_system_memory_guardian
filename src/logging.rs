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
//! This module contains the telemetry logging for the daemon.

use log::info;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use statslog_rust::memcg_memory_exceed_threshold_event::MemcgMemoryExceedThresholdEvent;

struct CgroupMemStat {
    anon_kilobytes: i32,
    file_kilobytes: i32,
}

fn read_cgroup_int(path: &Path) -> i32 {
    fs::read_to_string(path).ok().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(-1)
}

pub fn log_threshold_exceeded(uid: i32, process_name: &str, pid: i32) {
    // TODO(b/467035873) Support Non-System processes
    let cgroup_base = format!("/sys/fs/cgroup/system/uid_{}/pid_{}/", uid, pid);
    let mut stat_path_buf = PathBuf::from(&cgroup_base);
    stat_path_buf.push("memory.stat");

    let mem_stats = read_cgroup_memory_stat(&stat_path_buf);
    let swap_bytes = read_cgroup_int(Path::new(&format!("{}/memory.swap.current", cgroup_base)));

    // Convert bytes to KB
    let anon_kb = mem_stats.anon_kilobytes;
    let file_kb = mem_stats.file_kilobytes;
    let swap_kb = swap_bytes / 1024;

    info!(
        "MemcgMemoryExceedThresholdEvent anon_kb: {}, file_kb: {}, swap_kb: {}",
        anon_kb, file_kb, swap_kb,
    );
    let stats_atom = MemcgMemoryExceedThresholdEvent {
        uid,
        process_name,
        anon_memory_in_kilobytes: anon_kb,
        file_memory_in_kilobytes: file_kb,
        memory_swap_current_in_kilobytes: swap_kb,
    };
    let _ = stats_atom.stats_write();
}

fn read_cgroup_memory_stat(path: &Path) -> CgroupMemStat {
    let mut stats = CgroupMemStat { anon_kilobytes: 0, file_kilobytes: 0 };

    if let Ok(file) = File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let parts: Vec<&str> = line.split_whitespace().collect();

            if parts.len() >= 2 {
                let key = parts[0];
                let value = parts[1].parse::<u64>().unwrap_or(0);
                match key {
                    "anon" => stats.anon_kilobytes = (value / 1024) as i32,
                    "file" => stats.file_kilobytes = (value / 1024) as i32,
                    _ => {}
                }
            }
        }
    }
    stats
}
