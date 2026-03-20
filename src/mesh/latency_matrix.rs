// Copyright 2025 John A Keeney - Entrouter
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

/// Live latency matrix between all PoP nodes.
/// Updated by probe module, consumed by router for path selection.
/// Uses EWMA smoothing (α=0.125, matching TCP's RTT estimation).
use dashmap::DashMap;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Live latency measurement between a pair of nodes.
#[derive(Debug, Clone)]
pub struct LatencyEntry {
    pub smoothed_rtt: Duration,
    pub jitter: Duration,
    pub last_updated: Instant,
    pub samples: u64,
}

/// Thread-safe latency matrix tracking RTT between all node pairs.
/// Updated by the probe module and consumed by the mesh router.
pub struct LatencyMatrix {
    entries: DashMap<(String, String), LatencyEntry>,
}

impl Default for LatencyMatrix {
    fn default() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }
}

impl LatencyMatrix {
    /// Create an empty latency matrix.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update RTT for a path using EWMA smoothing (α=0.125)
    pub fn update(&self, from: &str, to: &str, rtt: Duration) {
        let key = (from.to_string(), to.to_string());
        self.entries
            .entry(key)
            .and_modify(|e| {
                let alpha = 0.125;
                let rtt_us = rtt.as_micros() as f64;
                let old_us = e.smoothed_rtt.as_micros() as f64;
                let new_us = old_us * (1.0 - alpha) + rtt_us * alpha;

                let diff = (rtt_us - old_us).abs();
                let old_jitter = e.jitter.as_micros() as f64;
                let new_jitter = old_jitter * (1.0 - alpha) + diff * alpha;

                e.smoothed_rtt = Duration::from_micros(new_us as u64);
                e.jitter = Duration::from_micros(new_jitter as u64);
                e.last_updated = Instant::now();
                e.samples += 1;
            })
            .or_insert_with(|| LatencyEntry {
                smoothed_rtt: rtt,
                jitter: Duration::ZERO,
                last_updated: Instant::now(),
                samples: 1,
            });
    }

    /// Get the smoothed RTT for a path
    pub fn get_rtt(&self, from: &str, to: &str) -> Option<Duration> {
        let key = (from.to_string(), to.to_string());
        self.entries.get(&key).map(|e| e.smoothed_rtt)
    }

    /// Get full entry for a path
    pub fn get_entry(&self, from: &str, to: &str) -> Option<LatencyEntry> {
        let key = (from.to_string(), to.to_string());
        self.entries.get(&key).map(|e| e.clone())
    }

    /// Return all edges as (from, to, rtt) for routing
    pub fn all_edges(&self) -> Vec<(String, String, Duration)> {
        self.entries
            .iter()
            .map(|e| {
                let (from, to) = e.key();
                (from.clone(), to.clone(), e.value().smoothed_rtt)
            })
            .collect()
    }

    /// Return all known node IDs
    pub fn nodes(&self) -> Vec<String> {
        let mut set = HashSet::new();
        for entry in self.entries.iter() {
            let (from, to) = entry.key();
            set.insert(from.clone());
            set.insert(to.clone());
        }
        set.into_iter().collect()
    }

    /// Number of tracked paths
    pub fn path_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_and_query() {
        let m = LatencyMatrix::new();
        m.update("syd", "sgp", Duration::from_millis(50));
        assert_eq!(m.get_rtt("syd", "sgp").unwrap(), Duration::from_millis(50));
        assert!(m.get_rtt("sgp", "syd").is_none());
    }

    #[test]
    fn ewma_smoothing() {
        let m = LatencyMatrix::new();
        m.update("a", "b", Duration::from_millis(100));
        m.update("a", "b", Duration::from_millis(200));
        let rtt = m.get_rtt("a", "b").unwrap();
        // After EWMA: 100 * 0.875 + 200 * 0.125 = 112.5ms
        assert!(rtt.as_millis() >= 112 && rtt.as_millis() <= 113);
    }

    #[test]
    fn all_edges_and_nodes() {
        let m = LatencyMatrix::new();
        m.update("syd", "sgp", Duration::from_millis(50));
        m.update("sgp", "lon", Duration::from_millis(80));
        assert_eq!(m.path_count(), 2);
        let nodes = m.nodes();
        assert!(nodes.contains(&"syd".to_string()));
        assert!(nodes.contains(&"sgp".to_string()));
        assert!(nodes.contains(&"lon".to_string()));
    }

    #[test]
    fn jitter_tracking() {
        let m = LatencyMatrix::new();
        m.update("a", "b", Duration::from_millis(100));
        // First update has no jitter (no previous value)
        let entry = m.entries.get(&("a".into(), "b".into())).unwrap();
        assert_eq!(entry.jitter, Duration::ZERO);
        drop(entry);

        // Second update: jitter = abs(200 - 100) = 100ms, smoothed by EWMA
        m.update("a", "b", Duration::from_millis(200));
        let entry = m.entries.get(&("a".into(), "b".into())).unwrap();
        assert!(entry.jitter > Duration::ZERO);
    }

    #[test]
    fn multiple_ewma_convergence() {
        let m = LatencyMatrix::new();
        // Push many identical samples - should converge
        for _ in 0..50 {
            m.update("x", "y", Duration::from_millis(100));
        }
        let rtt = m.get_rtt("x", "y").unwrap();
        assert_eq!(rtt.as_millis(), 100);
    }

    #[test]
    fn unknown_path_returns_none() {
        let m = LatencyMatrix::new();
        assert!(m.get_rtt("no", "path").is_none());
    }
}
