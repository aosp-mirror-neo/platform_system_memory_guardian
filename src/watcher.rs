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

use inotify::{Inotify, WatchDescriptor, WatchMask, Watches};
use log::error;
use nix::poll::PollTimeout;
use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags};
use std::os::fd::{AsFd, AsRawFd};

use super::MyError;
use super::MyResult;

use super::Process;

// An object that watches for changes in the system.
pub struct Watcher {
    pub inotify: Inotify,
    pub watcher: Watches,
    pub poller: Epoll,
}

impl Watcher {
    pub fn new() -> MyResult<Self> {
        // Create the inotify that will be used to watch for process events.
        let inotify = Inotify::init()?;
        let watcher = inotify.watches();

        // Watch the cgroup hierarchies for processes starting.  /proc is not suitable
        // because that file system does not play nice with inotify.
        let flags = WatchMask::CREATE;
        let _cs_wd = inotify.watches().add("/sys/fs/cgroup/system", flags);

        // Now create a poller to watch the inotify object, with a timeout.
        let ifd = inotify.as_fd();

        // Create an epoll instance and attach a single event, which is the inotify object.  The
        // "data" is just 1 (it's a don't-care here).
        let poller = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?;
        let ievent = EpollEvent::new(EpollFlags::EPOLLIN, 1);
        poller.add(ifd, ievent)?;

        Ok(Self { inotify, watcher, poller })
    }

    // Add the process to the watch list.  The poller will return an event when the process exits.
    pub fn add_process(&mut self, process: &Process) -> MyResult<()> {
        if let Some(fd) = &process.pfd {
            let pevent = EpollEvent::new(EpollFlags::EPOLLIN, fd.as_raw_fd() as u64);
            Ok(self.poller.add(fd, pevent)?)
        } else {
            Err(MyError::NotFound)
        }
    }

    // Remove the process from the watch list.
    pub fn delete_process(&mut self, process: &Process) {
        if let Some(fd) = &process.pfd {
            if let Err(e) = self.poller.delete(fd) {
                error!("failed to unregister process pid: {}", e);
            }
        }
    }

    // Add an inotify path.
    pub fn add_path(&mut self, path: &str, mask: WatchMask) -> MyResult<WatchDescriptor> {
        match self.watcher.add(path, mask) {
            Ok(wd) => Ok(wd),
            Err(e) => {
                error!("cannot watch path {}: {}", path, e);
                Err(MyError::Application { msg: e.to_string() })
            }
        }
    }

    // Remove an inotify path
    pub fn remove_path(&mut self, wd: WatchDescriptor) {
        if let Err(e) = self.watcher.remove(wd) {
            error!("failed to stop watching path: {}", e);
        }
    }

    pub fn wait(&self, events: &mut [EpollEvent], timeout: PollTimeout) -> MyResult<usize> {
        Ok(self.poller.wait(events, timeout)?)
    }
}
