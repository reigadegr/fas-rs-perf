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

use anyhow::Result;
use atoi::atoi;
use flume::{Receiver, Sender};
use hashbrown::{hash_map::Entry, HashMap};
use std::{
    cmp, fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
struct UsageTracker {
    tid: i32,
    last_cputime: u64,
    read_timer: Instant,
}

impl UsageTracker {
    fn new(tid: i32) -> Self {
        Self {
            tid,
            last_cputime: get_thread_cpu_time(tid),
            read_timer: Instant::now(),
        }
    }

    fn try_calculate(mut self) -> u64 {
        let tick_per_sec = 1_000_000_000.0;
        let new_cputime = get_thread_cpu_time(self.tid);
        let elapsed_ticks = self.read_timer.elapsed().as_secs_f64() * tick_per_sec;
        self.read_timer = Instant::now();
        let cputime_slice = new_cputime - self.last_cputime;
        self.last_cputime = new_cputime;
        (cputime_slice as f64 / elapsed_ticks) as u64
    }
}

#[derive(Debug)]
pub struct ProcessMonitor {
    stop: Arc<AtomicBool>,
    sender: Sender<Option<i32>>,
    util_max: Receiver<u64>,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        let (sender, receiver) = flume::bounded(0);
        let stop = Arc::new(AtomicBool::new(false));
        let (util_max_sender, util_max) = flume::unbounded();

        {
            let stop = stop.clone();

            thread::Builder::new()
                .name("ProcessMonitor".to_string())
                .spawn(move || {
                    monitor_thread(&stop, &receiver, &util_max_sender);
                })
                .unwrap();
        }

        Self {
            stop,
            sender,
            util_max,
        }
    }

    pub fn set_pid(&self, pid: Option<i32>) {
        self.sender.send(pid).unwrap();
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn update_util_max(&self) -> Option<u64> {
        self.util_max.try_iter().last()
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn monitor_thread(
    stop: &Arc<AtomicBool>,
    receiver: &Receiver<Option<i32>>,
    util_max: &Sender<u64>,
) {
    let mut current_pid = None;
    let mut last_full_update = Instant::now();
    let mut all_trackers = HashMap::new();
    let mut top_trackers = HashMap::new();

    while !stop.load(Ordering::Acquire) {
        if let Ok(pid) = receiver.try_recv() {
            current_pid = pid;
            all_trackers.clear();
            top_trackers.clear();
        }

        if let Some(pid) = current_pid {
            if last_full_update.elapsed() >= Duration::from_secs(1) {
                if let Ok(threads) = get_thread_ids(pid) {
                    all_trackers = threads
                        .iter()
                        .copied()
                        .map(|tid| {
                            (
                                tid,
                                match all_trackers.entry(tid) {
                                    Entry::Occupied(o) => o.remove(),
                                    Entry::Vacant(_) => UsageTracker::new(tid),
                                },
                            )
                        })
                        .collect();
                    let mut top_threads: Vec<_> = all_trackers
                        .iter()
                        .map(|(tid, tracker)| (*tid, (*tracker).try_calculate()))
                        .collect();

                    top_threads.sort_unstable_by(|(_, a), (_, b)| {
                        b.partial_cmp(a).unwrap_or(cmp::Ordering::Equal)
                    });
                    top_threads.truncate(5);
                    top_trackers = top_threads
                        .into_iter()
                        .map(|(tid, _)| match top_trackers.entry(tid) {
                            Entry::Occupied(o) => (tid, o.remove()),
                            Entry::Vacant(_) => (tid, UsageTracker::new(tid)),
                        })
                        .collect();

                    last_full_update = Instant::now();
                }
            }

            let mut max_usage: u64 = 0;
            for tracker in top_trackers.values_mut() {
                let usage = tracker.try_calculate();
                max_usage = max_usage.max(usage);
            }

            util_max.send(max_usage).unwrap();
        }

        thread::sleep(Duration::from_millis(300));
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

fn get_thread_cpu_time(tid: i32) -> u64 {
    let stat_path = format!("/proc/{tid}/schedstat");
    let stat_content = std::fs::read(stat_path).unwrap_or_else(|_| Vec::new());
    let mut parts = stat_content.split(|b| *b == b' ');
    let first_part = parts.next().unwrap_or_default();
    atoi::<u64>(first_part).unwrap_or(0)
}
