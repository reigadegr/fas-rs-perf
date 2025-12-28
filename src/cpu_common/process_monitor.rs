// Copyright 2025-2025, shadow3, shadow3aaa
//
// This file is part of fas-rs.
//
// fas-rs is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version.
//
// fas-rs is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more
// details.
//
// You should have received a copy of the GNU General Public License along
// with fas-rs. If not, see <https://www.gnu.org/licenses/>.
use std::{
    cmp,
    collections::{HashMap, hash_map::Entry},
    ffi::OsStr,
    fs::{self, File},
    io::{ErrorKind, Read},
    os::unix::ffi::OsStrExt,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use atoi::atoi;
use itoa::Buffer;
use libc::{_SC_CLK_TCK, sysconf};
use stringzilla::{stringzilla::StringZillableBinary, sz};

#[derive(Debug, Clone, Copy)]
struct UsageTracker {
    pid: i32,
    tid: i32,
    last_cputime: u64,
    read_timer: Instant,
    current_usage: f64,
}

impl UsageTracker {
    fn new(pid: i32, tid: i32) -> Result<Self> {
        Ok(Self {
            pid,
            tid,
            last_cputime: get_thread_cpu_time(pid, tid)?,
            read_timer: Instant::now(),
            current_usage: 0.0,
        })
    }

    fn try_calculate(&mut self) -> Result<f64> {
        let tick_per_sec = unsafe { sysconf(_SC_CLK_TCK) };
        let new_cputime = get_thread_cpu_time(self.pid, self.tid)?;
        let elapsed_ticks = self.read_timer.elapsed().as_secs_f64() * tick_per_sec as f64;
        self.read_timer = Instant::now();
        let cputime_slice = new_cputime - self.last_cputime;
        self.last_cputime = new_cputime;
        self.current_usage = cputime_slice as f64 / elapsed_ticks;
        Ok(self.current_usage)
    }
}

#[derive(Debug)]
pub struct ProcessMonitor {
    current_pid: Option<i32>,
    all_trackers: HashMap<i32, UsageTracker>,
    top_trackers: HashMap<i32, UsageTracker>,
    last_full_update: Instant,
    last_update: Instant,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        Self {
            current_pid: None,
            all_trackers: HashMap::new(),
            top_trackers: HashMap::new(),
            last_full_update: Instant::now(),
            last_update: Instant::now(),
        }
    }

    pub fn set_pid(&mut self, pid: Option<i32>) {
        if self.current_pid != pid {
            self.current_pid = pid;
            self.all_trackers.clear();
            self.top_trackers.clear();
            self.last_full_update = Instant::now();
            self.last_update = Instant::now();
        }
    }

    pub fn update(&mut self) -> Option<f64> {
        if self.last_update.elapsed() < Duration::from_millis(300) {
            return None;
        }

        self.last_update = Instant::now();
        let pid = self.current_pid?;

        if self.last_full_update.elapsed() >= Duration::from_secs(1) {
            self.update_thread_list(pid);
            self.last_full_update = Instant::now();
        }

        let mut util_max: f64 = 0.0;
        for tracker in self.top_trackers.values_mut() {
            if let Ok(usage) = tracker.try_calculate() {
                util_max = util_max.max(usage);
            }
        }

        Some(util_max)
    }

    fn update_thread_list(&mut self, pid: i32) {
        if let Ok(threads) = get_thread_ids(pid) {
            self.all_trackers = threads
                .iter()
                .copied()
                .filter_map(|tid| {
                    Some((
                        tid,
                        match self.all_trackers.entry(tid) {
                            Entry::Occupied(o) => o.remove(),
                            Entry::Vacant(_) => UsageTracker::new(pid, tid).ok()?,
                        },
                    ))
                })
                .collect();

            let mut top_threads: Vec<_> = self
                .all_trackers
                .iter()
                .filter_map(|(tid, tracker)| Some((*tid, tracker.clone().try_calculate().ok()?)))
                .collect();

            top_threads.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(cmp::Ordering::Equal));
            top_threads.truncate(8);

            self.top_trackers = top_threads
                .into_iter()
                .filter_map(|(tid, _)| match self.top_trackers.entry(tid) {
                    Entry::Occupied(o) => Some((tid, o.remove())),
                    Entry::Vacant(_) => Some((tid, UsageTracker::new(pid, tid).ok()?)),
                })
                .collect();
        }
    }

    pub fn top_threads(&self) -> impl Iterator<Item = i32> {
        self.top_trackers.keys().copied()
    }
}

fn get_thread_ids(pid: i32) -> Result<Vec<i32>> {
    let proc_path = format!("/proc/{pid}/task");
    Ok(fs::read_dir(proc_path)?
        .filter_map(|entry| {
            entry
                .ok()
                .and_then(|e| e.file_name().to_string_lossy().parse::<i32>().ok())
        })
        .collect())
}

pub fn get_stat_path<const N: usize>(pid: i32, tid: i32) -> [u8; N] {
    // /proc/
    let mut buffer = [0u8; N];
    let prefix = b"/proc/";
    buffer[..prefix.len()].copy_from_slice(prefix);

    // /proc/pid
    let mut itoa_buf = Buffer::new();
    let pid = itoa_buf.format(pid).as_bytes();

    let pid_length = pid.len();
    let prefix_len = prefix.len();
    buffer[prefix_len..prefix_len + pid_length].copy_from_slice(pid);

    // /proc/pid/task/
    let task_pos = prefix_len + pid_length;
    let task_path = b"/task/";
    buffer[task_pos..task_pos + task_path.len()].copy_from_slice(task_path);

    // /proc/pid/task/tid
    let tid_pos = task_pos + task_path.len();
    let tid = itoa_buf.format(tid).as_bytes();
    buffer[tid_pos..tid_pos + tid.len()].copy_from_slice(tid);

    // /proc/pid/task/tid/stat
    let stat_path = b"/stat";
    buffer[tid_pos + tid.len()..tid_pos + tid.len() + stat_path.len()].copy_from_slice(stat_path);
    buffer
}

pub fn read_to_byte<const N: usize>(file: &[u8]) -> Result<[u8; N]> {
    let end = sz::find(file, b"\0").unwrap_or(file.len());
    let file = &file[..end];
    let file = OsStr::from_bytes(file);
    let mut file = File::open(file).map_err(|e| anyhow!("Cannot open file: {e}"))?;

    let mut buffer = [0u8; N];
    match file.read_exact(&mut buffer) {
        Ok(()) => Ok(buffer),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(buffer),
        Err(e) => Err(e.into()),
    }
}

fn get_thread_cpu_time(pid: i32, tid: i32) -> Result<u64> {
    let stat_path = get_stat_path::<64>(pid, tid);
    let stat_content = read_to_byte::<1024>(&stat_path)?;
    let mut iter = stat_content.sz_splits(b" ").filter(|s| !s.is_empty());

    let utime_bytes = iter.nth(13).unwrap_or(&[0u8]);
    let stime_bytes = iter.next().unwrap_or(&[0u8]);

    let utime = atoi::<u64>(utime_bytes).unwrap_or(0);
    let stime = atoi::<u64>(stime_bytes).unwrap_or(0);
    Ok(utime + stime)
}
