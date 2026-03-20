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

/// Adaptive Forward Error Correction (FEC) using Reed-Solomon.
/// Dynamically adjusts parity ratio based on measured per-path loss rate.
///
/// Groups data into blocks of `data_shards` and generates `parity_shards`
/// recovery shards. Any `data_shards` of the total can reconstruct the block.
use reed_solomon_erasure::galois_8::ReedSolomon;

/// FEC configuration - adapts parity ratio based on observed loss.
#[derive(Debug, Clone, Copy)]
pub struct FecConfig {
    pub data_shards: usize,
    pub parity_shards: usize,
}

impl FecConfig {
    /// Choose FEC ratio based on measured loss rate (0.0 - 1.0).
    pub fn for_loss_rate(loss: f64) -> Self {
        match loss {
            l if l < 0.005 => Self {
                data_shards: 20,
                parity_shards: 1,
            }, // ~5% overhead
            l if l < 0.01 => Self {
                data_shards: 10,
                parity_shards: 2,
            }, // ~20% overhead
            l if l < 0.03 => Self {
                data_shards: 10,
                parity_shards: 4,
            }, // ~40% overhead
            l if l < 0.05 => Self {
                data_shards: 8,
                parity_shards: 4,
            }, // ~50% overhead
            _ => Self {
                data_shards: 6,
                parity_shards: 4,
            }, // ~67% overhead (heavy loss)
        }
    }

    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Overhead ratio (e.g. 0.2 = 20% bandwidth overhead).
    pub fn overhead(&self) -> f64 {
        self.parity_shards as f64 / self.data_shards as f64
    }
}

/// Reed-Solomon encoder/decoder for a fixed shard configuration.
pub struct FecEncoder {
    rs: ReedSolomon,
    pub config: FecConfig,
}

impl FecEncoder {
    /// Create an encoder for the given data/parity shard counts.
    pub fn new(config: FecConfig) -> Self {
        let rs =
            ReedSolomon::new(config.data_shards, config.parity_shards).expect("invalid FEC config");
        Self { rs, config }
    }

    /// Encode a block of data shards, producing parity shards.
    /// Input: `data_shards` Vec<Vec<u8>> all same length.
    /// Output: appends `parity_shards` parity vectors to the input.
    pub fn encode(&self, shards: &mut Vec<Vec<u8>>) {
        // Pad to total shards with empty parity buffers
        let shard_len = shards[0].len();
        while shards.len() < self.config.total_shards() {
            shards.push(vec![0u8; shard_len]);
        }
        self.rs.encode(shards).expect("FEC encode failed");
    }

    /// Reconstruct missing shards. Shards that are `None` are treated as lost.
    /// Returns Ok(()) if reconstruction succeeds, filling in the missing shards.
    pub fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<(), FecError> {
        self.rs
            .reconstruct(shards)
            .map_err(|_| FecError::TooManyLost)
    }
}

/// Track loss rate over a sliding window.
pub struct LossTracker {
    window: Vec<bool>, // true = received, false = lost
    pos: usize,
    count: usize,
}

impl LossTracker {
    pub fn new(window_size: usize) -> Self {
        Self {
            window: vec![true; window_size],
            pos: 0,
            count: 0,
        }
    }

    pub fn record(&mut self, received: bool) {
        if !self.window[self.pos] {
            // We're overwriting a lost entry, decrement loss count
            self.count = self.count.saturating_sub(1);
        }
        if !received {
            self.count += 1;
        }
        self.window[self.pos] = received;
        self.pos = (self.pos + 1) % self.window.len();
    }

    pub fn loss_rate(&self) -> f64 {
        self.count as f64 / self.window.len() as f64
    }

    /// Get the recommended FEC config based on current loss rate.
    pub fn recommended_config(&self) -> FecConfig {
        FecConfig::for_loss_rate(self.loss_rate())
    }
}

#[derive(Debug)]
pub enum FecError {
    TooManyLost,
}

impl std::fmt::Display for FecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FecError::TooManyLost => write!(f, "too many shards lost to reconstruct"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_no_loss() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let enc = FecEncoder::new(config);

        let mut shards: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8; 100]).collect();
        enc.encode(&mut shards);

        assert_eq!(shards.len(), 6);
        // All shards present
        let mut opt: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
        enc.reconstruct(&mut opt).unwrap();
    }

    #[test]
    fn recover_from_loss() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let enc = FecEncoder::new(config);

        let original: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8 + 10; 100]).collect();

        let mut shards = original.clone();
        enc.encode(&mut shards);

        // Lose 2 data shards (indices 0 and 2)
        let mut opt: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
        opt[0] = None;
        opt[2] = None;

        enc.reconstruct(&mut opt).unwrap();

        // Verify recovered data matches original
        assert_eq!(opt[0].as_ref().unwrap(), &original[0]);
        assert_eq!(opt[2].as_ref().unwrap(), &original[2]);
    }

    #[test]
    fn too_many_lost_fails() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let enc = FecEncoder::new(config);

        let mut shards: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8; 100]).collect();
        enc.encode(&mut shards);

        // Lose 3 shards (more than parity_shards=2)
        let mut opt: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
        opt[0] = None;
        opt[1] = None;
        opt[2] = None;

        assert!(enc.reconstruct(&mut opt).is_err());
    }

    #[test]
    fn loss_tracker_adapts() {
        let mut tracker = LossTracker::new(100);

        // No loss → minimal FEC
        for _ in 0..100 {
            tracker.record(true);
        }
        let config = tracker.recommended_config();
        assert_eq!(config.parity_shards, 1);

        // 5% loss → heavy FEC
        let mut tracker2 = LossTracker::new(100);
        for i in 0..100 {
            tracker2.record(i % 20 != 0); // lose every 20th = 5%
        }
        let config2 = tracker2.recommended_config();
        assert!(config2.parity_shards >= 4);
    }

    #[test]
    fn adaptive_config_thresholds() {
        assert_eq!(FecConfig::for_loss_rate(0.0).parity_shards, 1);
        assert_eq!(FecConfig::for_loss_rate(0.008).parity_shards, 2);
        assert_eq!(FecConfig::for_loss_rate(0.02).parity_shards, 4);
        assert_eq!(FecConfig::for_loss_rate(0.04).parity_shards, 4);
        assert_eq!(FecConfig::for_loss_rate(0.10).parity_shards, 4);
    }

    #[test]
    fn adaptive_config_all_boundaries() {
        // Right at each boundary
        let t1 = FecConfig::for_loss_rate(0.004);
        assert_eq!((t1.data_shards, t1.parity_shards), (20, 1));

        let t2 = FecConfig::for_loss_rate(0.005); // crosses into tier 2
        assert_eq!((t2.data_shards, t2.parity_shards), (10, 2));

        let t3 = FecConfig::for_loss_rate(0.01); // crosses into tier 3
        assert_eq!((t3.data_shards, t3.parity_shards), (10, 4));

        let t4 = FecConfig::for_loss_rate(0.03); // crosses into tier 4
        assert_eq!((t4.data_shards, t4.parity_shards), (8, 4));

        let t5 = FecConfig::for_loss_rate(0.05); // crosses into tier 5
        assert_eq!((t5.data_shards, t5.parity_shards), (6, 4));
    }

    #[test]
    fn overhead_calculation() {
        let c = FecConfig {
            data_shards: 10,
            parity_shards: 2,
        };
        assert!((c.overhead() - 0.2).abs() < f64::EPSILON);

        let c2 = FecConfig {
            data_shards: 6,
            parity_shards: 4,
        };
        assert!((c2.overhead() - 4.0 / 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn total_shards() {
        let c = FecConfig {
            data_shards: 10,
            parity_shards: 4,
        };
        assert_eq!(c.total_shards(), 14);
    }

    #[test]
    fn loss_tracker_window_wraparound() {
        let mut tracker = LossTracker::new(4);
        // Fill window: [recv, lost, recv, lost] → 50% loss
        tracker.record(true);
        tracker.record(false);
        tracker.record(true);
        tracker.record(false);
        assert!((tracker.loss_rate() - 0.5).abs() < f64::EPSILON);

        // Wrap around - overwrite the first two:  [recv, recv, recv, lost] → 25%
        tracker.record(true);
        tracker.record(true);
        assert!((tracker.loss_rate() - 0.25).abs() < f64::EPSILON);
    }
}
