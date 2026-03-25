// Copyright 2026, The Android Open Source Project
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

//! This module provides a mockable interface for OS/filesystem operations.

use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg_attr(any(test, feature = "test_utils"), mockall::automock)]
pub trait FsApi {
    fn read_to_string(path: &str) -> io::Result<String>;

    fn list_dir(path: &str) -> io::Result<Vec<String>>;

    fn get_uid(path: &str) -> io::Result<u32>;
}

#[allow(dead_code)]
pub struct FsApiImpl;

impl FsApi for FsApiImpl {
    fn read_to_string(path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn list_dir(path: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    fn get_uid(path: &str) -> io::Result<u32> {
        let meta = std::fs::metadata(path)?;
        #[cfg(unix)]
        {
            Ok(meta.uid())
        }
        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }
}

#[cfg_attr(any(test, feature = "test_utils"), mockall::automock)]
pub trait ProcessGroupApi {
    fn kill_process_group(uid: u32, pid: i32, signal: i32)
        -> std::result::Result<(), &'static str>;
    fn set_process_profiles(
        uid: u32,
        pid: i32,
        profiles: Vec<String>,
    ) -> std::result::Result<(), &'static str>;
}

pub struct ProcessGroupApiImpl;

impl ProcessGroupApi for ProcessGroupApiImpl {
    fn kill_process_group(
        uid: u32,
        pid: i32,
        signal: i32,
    ) -> std::result::Result<(), &'static str> {
        processgroup::processgroup::kill_process_group(uid, pid, signal)
    }

    fn set_process_profiles(
        uid: u32,
        pid: i32,
        profiles: Vec<String>,
    ) -> std::result::Result<(), &'static str> {
        let profiles_ref: Vec<&str> = profiles.iter().map(|s| s.as_str()).collect();
        processgroup::processgroup::set_process_profiles(uid, pid, &profiles_ref)
    }
}
