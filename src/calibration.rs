//! Calibration data types — per-string onset/release thresholds.
//!
//! This module defines the serializable calibration format and is always compiled.
//! The interactive `Calibrator` tool lives in `calibrator.rs` (behind the `calibration` feature).

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringThreshold {
    pub onset: f64,
    pub release: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calibration {
    pub strings: Vec<StringThreshold>,
}

impl Calibration {
    /// Load from a JSON file. Returns None if file is absent or malformed.
    pub fn load(path: &std::path::Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&data) {
            Ok(c) => {
                info!("Loaded calibration from {:?}", path);
                Some(c)
            }
            Err(e) => {
                warn!("Failed to parse calibration file {:?}: {}", path, e);
                None
            }
        }
    }

    pub fn save(&self, path: &std::path::Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, json)?;
        info!("Calibration saved to {:?}", path);
        Ok(())
    }

    /// Per-string onset thresholds as a flat array for StringDetector.
    pub fn onset_thresholds(&self) -> [f64; 10] {
        let mut t = [0.02f64; 10];
        for (i, s) in self.strings.iter().take(10).enumerate() {
            t[i] = s.onset;
        }
        t
    }

    /// Per-string release thresholds as a flat array for StringDetector.
    pub fn release_thresholds(&self) -> [f64; 10] {
        let mut t = [0.008f64; 10];
        for (i, s) in self.strings.iter().take(10).enumerate() {
            t[i] = s.release;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calibration_roundtrip() {
        let cal = Calibration {
            strings: (0..10)
                .map(|i| StringThreshold {
                    onset: 0.02 + i as f64 * 0.001,
                    release: 0.008 + i as f64 * 0.0004,
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&cal).unwrap();
        let loaded: Calibration = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.strings.len(), 10);
        assert!((loaded.strings[3].onset - cal.strings[3].onset).abs() < 1e-10);
    }
}
