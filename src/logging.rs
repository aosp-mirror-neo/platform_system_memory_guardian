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

use crate::memcg_proc_memory_util::CgroupMemStat;

use statslog_rust::memcg_memory_exceed_threshold_event::MemcgMemoryExceedThresholdEvent;

pub fn log_threshold_exceeded(
    uid: i32,
    process_name: &str,
    mem_stats: &CgroupMemStat,
    swap_kb: u32,
) {
    let anon_kb = mem_stats.anon_kilobytes;
    let file_kb = mem_stats.file_kilobytes;
    info!(
        "MemcgMemoryExceedThresholdEvent anon_kb: {}, file_kb: {}, swap_kb: {}",
        anon_kb, file_kb, swap_kb,
    );
    let stats_atom = MemcgMemoryExceedThresholdEvent {
        uid,
        process_name,
        anon_memory_in_kilobytes: anon_kb as i32,
        file_memory_in_kilobytes: file_kb as i32,
        memory_swap_current_in_kilobytes: swap_kb as i32,
    };
    let _ = stats_atom.stats_write();
}
