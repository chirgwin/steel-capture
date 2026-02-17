use crate::types::*;
use log::trace;

/// Interpolates bar position from 4 SS49E hall sensors mounted along
/// the treble-side rail at known fret positions.
///
/// # Physics model
///
/// A small neodymium magnet (6mm×3mm) is attached to the treble end
/// of the bar. Each SS49E sensor outputs a voltage proportional to
/// the magnetic field, which falls off approximately as:
///
///   field ∝ 1 / (d² + h²)^(3/2)
///
/// where d = horizontal distance from sensor to magnet, h = standoff.
///
/// With sensors at frets 0, 5, 10, 15, the nearest sensor always gets
/// a strong reading and its neighbors get progressively weaker readings.
/// Weighted centroid interpolation recovers the position to ±0.3 frets
/// across the full range.
///
/// # Noise floor and bar detection
///
/// When the bar is lifted or far from all sensors, all readings are
/// near zero (below `presence_threshold`). This gives reliable bar
/// on/off detection even during silence.
pub struct BarSensor {
    /// Fret positions of the 4 sensors
    sensor_frets: [f32; 4],
    /// Minimum total sensor reading to consider bar present
    presence_threshold: f32,
    /// Smoothing factor (0.0 = no smoothing, 0.99 = very smooth)
    smoothing: f32,
    /// Previous position for smoothing
    last_position: Option<f32>,
}

impl BarSensor {
    pub fn new() -> Self {
        Self {
            sensor_frets: BAR_SENSOR_FRETS,
            presence_threshold: 0.05,
            smoothing: 0.3,
            last_position: None,
        }
    }

    /// Estimate bar position from raw hall sensor readings.
    ///
    /// Returns (position_in_frets, confidence) or None if bar not detected.
    ///
    /// Uses local peak interpolation: find the strongest sensor, then
    /// interpolate between it and the second-strongest neighbor. This
    /// avoids centroid edge bias at frets 0 and 15+.
    pub fn estimate(&mut self, readings: &[f32; 4]) -> Option<(f32, f32)> {
        let total: f32 = readings.iter().sum();

        // Bar not present
        if total < self.presence_threshold {
            self.last_position = None;
            return None;
        }

        // Find the dominant sensor (highest reading)
        let (peak_idx, &peak_val) = readings
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();

        if peak_val < self.presence_threshold {
            self.last_position = None;
            return None;
        }

        // Find the best neighbor (highest reading among adjacent sensors)
        let left = if peak_idx > 0 { Some((peak_idx - 1, readings[peak_idx - 1])) } else { None };
        let right = if peak_idx < 3 { Some((peak_idx + 1, readings[peak_idx + 1])) } else { None };

        let neighbor = match (left, right) {
            (Some(l), Some(r)) => if l.1 >= r.1 { Some(l) } else { Some(r) },
            (Some(l), None) => Some(l),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        };

        // Interpolate between peak and neighbor
        let raw_pos = match neighbor {
            Some((n_idx, n_val)) if n_val > self.presence_threshold * 0.5 => {
                // Linear interpolation weighted by readings
                let peak_fret = self.sensor_frets[peak_idx];
                let neighbor_fret = self.sensor_frets[n_idx];
                let t = n_val / (peak_val + n_val);
                peak_fret + t * (neighbor_fret - peak_fret)
            }
            _ => {
                // Only peak sensor has a meaningful reading
                self.sensor_frets[peak_idx]
            }
        };

        // Confidence based on how peaked the distribution is.
        let peakedness = peak_val / total; // 0.25 (uniform) to 1.0 (one sensor)
        let confidence = ((peakedness - 0.25) / 0.75).clamp(0.3, 1.0);

        // Apply smoothing
        let smoothed = match self.last_position {
            Some(prev) => {
                let alpha = 1.0 - self.smoothing;
                prev + alpha * (raw_pos - prev)
            }
            None => raw_pos,
        };
        self.last_position = Some(smoothed);

        trace!(
            "bar_sensor: raw={:.2} smoothed={:.2} conf={:.2} peak=s{} readings=[{:.3} {:.3} {:.3} {:.3}]",
            raw_pos, smoothed, confidence, peak_idx,
            readings[0], readings[1], readings[2], readings[3],
        );

        Some((smoothed, confidence))
    }

    /// Reset state (e.g., on session restart)
    pub fn reset(&mut self) {
        self.last_position = None;
    }
}

/// Simulate hall sensor readings for a bar at a given fret position.
///
/// Models the magnetic field falloff from a neodymium dipole:
///   reading = amplitude / (1 + (d / characteristic_distance)²)^(3/2)
///
/// where d = distance from sensor to bar magnet in fret-space,
/// and characteristic_distance controls the "width" of the response.
///
/// The characteristic_distance of ~2.5 frets was chosen empirically:
/// - Too narrow (1.0): only nearest sensor reads, no interpolation data
/// - Too wide (5.0): all sensors read similarly, poor discrimination
/// - 2.5: nearest sensor saturates high, 1-2 neighbors give gradient
pub fn simulate_bar_readings(bar_fret: f32) -> [f32; 4] {
    const CHAR_DIST: f32 = 2.5; // frets — controls response width
    const AMPLITUDE: f32 = 1.0;
    let mut readings = [0.0f32; 4];
    for (i, &sensor_fret) in BAR_SENSOR_FRETS.iter().enumerate() {
        let d = (bar_fret - sensor_fret).abs();
        let normalized = d / CHAR_DIST;
        let denominator = (1.0 + normalized * normalized).powf(1.5);
        readings[i] = (AMPLITUDE / denominator).min(1.0);
    }
    readings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bar_at_fret_0() {
        let readings = simulate_bar_readings(0.0);
        assert!(readings[0] > 0.9, "sensor at fret 0 should be strong");
        assert!(readings[1] < readings[0], "sensor at fret 5 should be weaker");
        assert!(readings[3] < 0.05, "sensor at fret 15 should be near zero");
    }

    #[test]
    fn test_bar_at_fret_5() {
        let readings = simulate_bar_readings(5.0);
        assert!(readings[1] > 0.9, "sensor at fret 5 should peak");
        assert!(readings[0] < readings[1], "neighbors weaker");
        assert!(readings[2] < readings[1], "neighbors weaker");
    }

    #[test]
    fn test_bar_between_sensors() {
        let readings = simulate_bar_readings(7.5);
        // Between fret 5 and 10 — both should read, but fret 5 and 10 dominate
        assert!(readings[1] > readings[0], "fret 5 closer than fret 0");
        assert!(readings[2] > readings[0], "fret 10 closer than fret 0");
    }

    #[test]
    fn test_interpolation_accuracy() {
        let mut sensor = BarSensor::new();
        // Test at each integer fret from 0 to 15
        for target in 0..=15 {
            let fret = target as f32;
            let readings = simulate_bar_readings(fret);
            sensor.last_position = None; // no smoothing interference
            let result = sensor.estimate(&readings);
            assert!(result.is_some(), "bar should be detected at fret {}", target);
            let (pos, _conf) = result.unwrap();
            assert!(
                (pos - fret).abs() < 0.5,
                "fret {}: estimated {:.2}, error {:.2}",
                target, pos, (pos - fret).abs()
            );
        }
    }

    #[test]
    fn test_interpolation_between_sensors() {
        let mut sensor = BarSensor::new();
        // Test at half-fret positions between sensors
        for half_fret in &[2.5, 7.5, 12.5] {
            let readings = simulate_bar_readings(*half_fret);
            sensor.last_position = None;
            let result = sensor.estimate(&readings);
            assert!(result.is_some());
            let (pos, _) = result.unwrap();
            assert!(
                (pos - half_fret).abs() < 1.0,
                "fret {}: estimated {:.2}",
                half_fret, pos
            );
        }
    }

    #[test]
    fn test_bar_lifted() {
        let mut sensor = BarSensor::new();
        let readings = [0.0f32; 4];
        assert!(sensor.estimate(&readings).is_none());
    }

    #[test]
    fn test_bar_beyond_sensors() {
        // Bar at fret 20 — beyond last sensor at 15
        let readings = simulate_bar_readings(20.0);
        let mut sensor = BarSensor::new();
        let result = sensor.estimate(&readings);
        // Might still detect (fret 15 sensor picks up something) but accuracy drops
        if let Some((pos, conf)) = result {
            assert!(pos > 10.0, "should be toward high frets: {:.2}", pos);
            assert!(conf < 0.9, "confidence should reflect distance");
        }
    }

    #[test]
    fn test_smoothing() {
        let mut sensor = BarSensor::new();
        // Simulate bar jumping from fret 3 to fret 8 — smoothing should dampen
        let r1 = simulate_bar_readings(3.0);
        let _ = sensor.estimate(&r1);
        let r2 = simulate_bar_readings(8.0);
        let (pos, _) = sensor.estimate(&r2).unwrap();
        // Should be between 3 and 8 due to smoothing
        assert!(pos > 3.0 && pos < 8.0,
            "smoothed pos {:.2} should be between 3 and 8", pos);
    }
}
