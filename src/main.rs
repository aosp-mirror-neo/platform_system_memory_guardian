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

//! pmg (process memory guardian) is a native daemon monitoring memcg memory for a process.

use log::info;
use log::LevelFilter;

fn main() {
    let _init_success = logger::init(
        logger::Config::default().with_tag_on_device("pmg").with_max_level(LevelFilter::Trace),
    );
    if !pmg_flags::memory_guardian_enabled() {
        info!("pmg is disabled");
        return;
    }
    info!("pmg started");
}
