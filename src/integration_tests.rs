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

#[cfg(test)]
mod tests {
    use crate::config::Specification;
    use crate::os_api::MockFsApi;
    use crate::os_api::MockProcessGroupApi;
    use crate::utils::Process;

    #[test]
    fn test_process_discovery_mocked() {
        let list_dir_ctx = MockFsApi::list_dir_context();
        list_dir_ctx.expect().returning(|path| {
            if path == "/proc" {
                Ok(vec!["1234".to_string(), "5678".to_string()])
            } else {
                Ok(vec![])
            }
        });

        let get_uid_ctx = MockFsApi::get_uid_context();
        get_uid_ctx.expect().returning(|path| {
            if path == "/proc/1234" {
                Ok(1000)
            } else if path == "/proc/5678" {
                Ok(2000)
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
            }
        });

        let read_string_ctx = MockFsApi::read_to_string_context();
        read_string_ctx.expect().returning(|path| {
            if path == "/proc/1234/cmdline" {
                Ok("system_server\0".to_string())
            } else if path == "/proc/5678/cmdline" {
                Ok("com.android.systemui\0".to_string())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
            }
        });

        let spec = Specification {
            target_cmd: "system_server".to_string(),
            uid: Some(1000),
            reclaim_wait_time_secs: 5,
            profile_info: crate::config::ProfileConfig::Both {
                mem_limit_profile: "SystemServerMemoryHighLimit".to_string(),
                additional_task_profiles: vec![],
                anon_limit_in_mb: 500,
            },
        };

        let process = Process::from(&spec).unwrap();
        assert_eq!(process.pid, 1234);
        assert_eq!(process.uid, 1000);
    }

    #[test]
    fn test_handle_memory_high_event_kill_success_kill_due_to_high_anon_memory() {
        let read_to_string_ctx = MockFsApi::read_to_string_context();
        read_to_string_ctx.expect().returning(|path| {
            if path.contains("memory.stat") {
                Ok("anon 104857600\nfile 0\n".to_string()) // 100MB
            } else if path.contains("memory.swap.current") {
                Ok("0\n".to_string())
            } else if path.contains("memory.high") {
                Ok("52428800\n".to_string()) // 50MB
            } else if path.contains("memory.current") {
                Ok("104857600\n".to_string()) // 100MB
            } else if path.contains("cmdline") {
                Ok("test_process".to_string()) // No newline or nulls
            } else {
                Ok("0\n".to_string())
            }
        });

        let list_dir_ctx = MockFsApi::list_dir_context();
        list_dir_ctx.expect().returning(|path| {
            if path == "/proc" {
                Ok(vec!["1234".to_string()])
            } else {
                Ok(vec![])
            }
        });

        let get_uid_ctx = MockFsApi::get_uid_context();
        get_uid_ctx.expect().returning(|_path| Ok(1000));

        // Setup mock for process group operations
        let kill_ctx = MockProcessGroupApi::kill_process_group_context();
        kill_ctx.expect().times(1).returning(|_uid, _pid, _signal| Ok(()));

        // Create Specification
        let spec = Specification {
            target_cmd: "test_process".to_string(),
            uid: Some(1000),
            reclaim_wait_time_secs: 1,
            profile_info: crate::config::ProfileConfig::Both {
                mem_limit_profile: "TestProfile".to_string(),
                additional_task_profiles: vec![],
                anon_limit_in_mb: 50,
            },
        };

        let process = Process::from(&spec).unwrap();
        let mut monitored_process = crate::MonitoredProcess {
            spec,
            process: Some(process),
            monitor: None,
            found: true,
            handled: false,
            ignore: false,
        };

        let result = crate::handle_memory_high_event::<MockProcessGroupApi>(&mut monitored_process);

        assert_eq!(result, crate::HighMemoryEventResult::KilledDueToHighAnonMemory);
    }
}
