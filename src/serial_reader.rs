use crate::types::*;
use byteorder::{LittleEndian, ReadBytesExt};
use crossbeam_channel::Sender;
use log::{debug, error, info, warn};
use std::io::{self, Cursor, Read};
use std::time::Duration;

/// Binary protocol from Teensy:
///
/// | Offset | Size | Field        |
/// |--------|------|--------------|
/// | 0      | 2    | sync (0xBEEF)|
/// | 2      | 4    | timestamp_us (u32, wrapping) |
/// | 6      | 2×13 | ADC values (u16 × 13 channels) |
/// | 32     | 2    | CRC16        |
/// | Total: 34 bytes              |
///
/// Channel order: A0=pedal_A, A1=pedal_B, A2=pedal_C,
///   A3=LKL, A4=LKR, A5=LKV, A6=RKL, A7=RKR, A8=volume,
///   A9=bar_fret0, A10=bar_fret5, A11=bar_fret10, A12=bar_fret15
const FRAME_SIZE: usize = 34;
const SYNC_WORD: u16 = 0xBEEF;
const NUM_CHANNELS: usize = 13;

/// Calibration: maps raw ADC (0–4095 for Teensy's 12-bit ADC) to 0.0–1.0.
/// Each channel has min/max raw values.
#[derive(Clone)]
pub struct Calibration {
    /// (min_raw, max_raw) for each of 13 channels
    pub ranges: [(u16, u16); NUM_CHANNELS],
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            // Default range: 200–3800 out of 0–4095 (Teensy 12-bit ADC).
            // Margins avoid noise near rails: SS49E outputs ~0.2V at rest
            // (ADC ~200) and most hall/pot sensors don't reach full 3.3V
            // (ADC ~3800). Real calibration should replace these per-channel
            // by observing actual sensor values at rest and fully engaged.
            ranges: [(200, 3800); NUM_CHANNELS],
        }
    }
}

pub struct SerialReader {
    port_name: String,
    baud_rate: u32,
    tx: Sender<InputEvent>,
    clock: SessionClock,
    calibration: Calibration,
}

impl SerialReader {
    pub fn new(port_name: String, tx: Sender<InputEvent>, clock: SessionClock) -> Self {
        Self {
            port_name,
            baud_rate: 115200,
            tx,
            clock,
            calibration: Calibration::default(),
        }
    }

    pub fn with_calibration(mut self, cal: Calibration) -> Self {
        self.calibration = cal;
        self
    }

    /// Run the serial reader loop. Blocks the calling thread.
    pub fn run(&self) {
        info!(
            "Opening serial port: {} @ {}",
            self.port_name, self.baud_rate
        );

        let port = serialport::new(&self.port_name, self.baud_rate)
            .timeout(Duration::from_millis(100))
            .open();

        let mut port = match port {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to open serial port {}: {}", self.port_name, e);
                error!("Is the Teensy connected? Run with --simulate for dev mode.");
                return;
            }
        };

        info!("Serial port opened. Reading frames...");
        let mut buf = [0u8; 256];
        let mut frame_buf = Vec::with_capacity(FRAME_SIZE * 4);
        let mut frame_count: u64 = 0;
        let mut error_count: u64 = 0;

        loop {
            match port.read(&mut buf) {
                Ok(n) => {
                    frame_buf.extend_from_slice(&buf[..n]);

                    // Process all complete frames in the buffer
                    while frame_buf.len() >= FRAME_SIZE {
                        // Find sync word
                        if let Some(sync_pos) = find_sync(&frame_buf) {
                            if sync_pos > 0 {
                                // Discard bytes before sync
                                debug!("Skipping {} bytes to sync", sync_pos);
                                frame_buf.drain(..sync_pos);
                            }
                            if frame_buf.len() < FRAME_SIZE {
                                break;
                            }

                            let frame_bytes: Vec<u8> = frame_buf.drain(..FRAME_SIZE).collect();

                            match parse_frame(&frame_bytes, &self.calibration, &self.clock) {
                                Ok(sensor) => {
                                    let _ = self.tx.send(InputEvent::Sensor(sensor));
                                    frame_count += 1;
                                    if frame_count.is_multiple_of(5000) {
                                        info!(
                                            "Serial: {} frames, {} errors",
                                            frame_count, error_count
                                        );
                                    }
                                }
                                Err(e) => {
                                    error_count += 1;
                                    debug!("Frame parse error: {}", e);
                                }
                            }
                        } else {
                            // No sync found — discard all but last byte
                            let keep = frame_buf.len().saturating_sub(1);
                            frame_buf.drain(..keep);
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => {
                    warn!("Serial read error: {}", e);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

fn find_sync(buf: &[u8]) -> Option<usize> {
    (0..buf.len().saturating_sub(1)).find(|&i| buf[i] == 0xEF && buf[i + 1] == 0xBE)
}

fn parse_frame(
    data: &[u8],
    cal: &Calibration,
    clock: &SessionClock,
) -> Result<SensorFrame, String> {
    if data.len() != FRAME_SIZE {
        return Err(format!("wrong size: {}", data.len()));
    }

    let mut cursor = Cursor::new(data);

    // Sync word
    let sync = cursor
        .read_u16::<LittleEndian>()
        .map_err(|e| e.to_string())?;
    if sync != SYNC_WORD {
        return Err(format!("bad sync: 0x{:04X}", sync));
    }

    // Timestamp from Teensy (u32 microseconds, wrapping)
    let _teensy_ts = cursor
        .read_u32::<LittleEndian>()
        .map_err(|e| e.to_string())?;

    // ADC values (13 channels, u16 each)
    let mut raw = [0u16; NUM_CHANNELS];
    for ch in raw.iter_mut() {
        *ch = cursor
            .read_u16::<LittleEndian>()
            .map_err(|e| e.to_string())?;
    }

    // CRC16
    let received_crc = cursor
        .read_u16::<LittleEndian>()
        .map_err(|e| e.to_string())?;
    let computed_crc = crc16(&data[..FRAME_SIZE - 2]);
    if received_crc != computed_crc {
        return Err(format!(
            "CRC mismatch: received 0x{:04X}, computed 0x{:04X}",
            received_crc, computed_crc
        ));
    }

    // Calibrate: map raw ADC to 0.0–1.0
    let mut calibrated = [0.0f32; NUM_CHANNELS];
    for i in 0..NUM_CHANNELS {
        let (lo, hi) = cal.ranges[i];
        let range = (hi as f32 - lo as f32).max(1.0);
        calibrated[i] = ((raw[i] as f32 - lo as f32) / range).clamp(0.0, 1.0);
    }

    // Use host clock for consistent timestamps (Teensy clock may drift)
    let timestamp_us = clock.now_us();

    Ok(SensorFrame {
        timestamp_us,
        pedals: [calibrated[0], calibrated[1], calibrated[2]],
        knee_levers: [
            calibrated[3],
            calibrated[4],
            calibrated[5],
            calibrated[6],
            calibrated[7],
        ],
        volume: calibrated[8],
        bar_sensors: [
            calibrated[9],
            calibrated[10],
            calibrated[11],
            calibrated[12],
        ],
        // Hardware doesn't know which strings are picked — audio detection handles this.
        string_active: [false; 10],
    })
}

/// CRC-16/CCITT-FALSE
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid 34-byte frame with correct CRC.
    fn make_frame(adc_values: &[u16; NUM_CHANNELS], timestamp: u32) -> Vec<u8> {
        let mut buf = vec![0u8; FRAME_SIZE];
        // Sync
        buf[0] = (SYNC_WORD & 0xFF) as u8;
        buf[1] = (SYNC_WORD >> 8) as u8;
        // Timestamp
        buf[2] = (timestamp & 0xFF) as u8;
        buf[3] = ((timestamp >> 8) & 0xFF) as u8;
        buf[4] = ((timestamp >> 16) & 0xFF) as u8;
        buf[5] = ((timestamp >> 24) & 0xFF) as u8;
        // ADC values
        for i in 0..NUM_CHANNELS {
            buf[6 + i * 2] = (adc_values[i] & 0xFF) as u8;
            buf[6 + i * 2 + 1] = (adc_values[i] >> 8) as u8;
        }
        // CRC over first 32 bytes
        let crc = crc16(&buf[..FRAME_SIZE - 2]);
        buf[FRAME_SIZE - 2] = (crc & 0xFF) as u8;
        buf[FRAME_SIZE - 1] = (crc >> 8) as u8;
        buf
    }

    #[test]
    fn test_crc16() {
        let data = b"123456789";
        let crc = crc16(data);
        assert_eq!(crc, 0x29B1, "CRC-16/CCITT-FALSE of '123456789'");
    }

    #[test]
    fn test_find_sync() {
        let buf = [0x00, 0x00, 0xEF, 0xBE, 0x01, 0x02];
        assert_eq!(find_sync(&buf), Some(2));
    }

    #[test]
    fn test_find_sync_at_start() {
        let buf = [0xEF, 0xBE, 0x01, 0x02];
        assert_eq!(find_sync(&buf), Some(0));
    }

    #[test]
    fn test_find_sync_not_found() {
        let buf = [0x00, 0x01, 0x02, 0x03];
        assert_eq!(find_sync(&buf), None);
    }

    #[test]
    fn test_find_sync_partial_at_end() {
        // 0xEF at last byte — can't confirm sync, should return None
        let buf = [0x00, 0x01, 0xEF];
        assert_eq!(find_sync(&buf), None);
    }

    #[test]
    fn test_find_sync_empty() {
        assert_eq!(find_sync(&[]), None);
        assert_eq!(find_sync(&[0xEF]), None);
    }

    #[test]
    fn test_parse_valid_frame() {
        let adc = [2000u16; NUM_CHANNELS];
        let frame = make_frame(&adc, 1000);
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let result = parse_frame(&frame, &cal, &clock);
        assert!(result.is_ok());
        let sf = result.unwrap();
        // With default cal (200, 3800), raw 2000 → (2000-200)/3600 ≈ 0.5
        assert!((sf.pedals[0] - 0.5).abs() < 0.01);
        assert!((sf.volume - 0.5).abs() < 0.01);
        assert!((sf.bar_sensors[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_parse_frame_bad_crc() {
        let adc = [2000u16; NUM_CHANNELS];
        let mut frame = make_frame(&adc, 1000);
        frame[FRAME_SIZE - 1] ^= 0xFF;
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let result = parse_frame(&frame, &cal, &clock);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CRC mismatch"));
    }

    #[test]
    fn test_parse_frame_bad_sync() {
        let adc = [2000u16; NUM_CHANNELS];
        let mut frame = make_frame(&adc, 1000);
        frame[0] = 0x00;
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let result = parse_frame(&frame, &cal, &clock);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bad sync"));
    }

    #[test]
    fn test_parse_frame_wrong_size() {
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let result = parse_frame(&[0u8; 10], &cal, &clock);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("wrong size"));
    }

    #[test]
    fn test_calibration_clamps() {
        let mut adc = [0u16; NUM_CHANNELS];
        adc[0] = 0; // below min (200) → clamps to 0.0
        adc[1] = 4095; // above max (3800) → clamps to 1.0
        adc[2] = 200; // exactly at min → 0.0
        let frame = make_frame(&adc, 500);
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let sf = parse_frame(&frame, &cal, &clock).unwrap();
        assert_eq!(sf.pedals[0], 0.0, "below min clamps to 0");
        assert_eq!(sf.pedals[1], 1.0, "above max clamps to 1");
        assert_eq!(sf.pedals[2], 0.0, "exactly at min = 0");
    }

    #[test]
    fn test_find_sync_with_garbage() {
        let mut buf = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        buf.push(0xEF);
        buf.push(0xBE);
        buf.push(0x00);
        assert_eq!(find_sync(&buf), Some(5));
    }

    #[test]
    fn test_channel_mapping() {
        let mut adc = [0u16; NUM_CHANNELS];
        adc[0] = 3800;
        adc[1] = 3800;
        adc[2] = 3800; // pedals → 1.0
        adc[3] = 200;
        adc[4] = 200;
        adc[5] = 200;
        adc[6] = 200;
        adc[7] = 200; // levers → 0.0
        adc[8] = 2000; // volume → ~0.5
        adc[9] = 3000;
        adc[10] = 3000;
        adc[11] = 3000;
        adc[12] = 3000; // bar → ~0.78
        let frame = make_frame(&adc, 0);
        let cal = Calibration::default();
        let clock = SessionClock::new();
        let sf = parse_frame(&frame, &cal, &clock).unwrap();
        assert!((sf.pedals[0] - 1.0).abs() < 0.01);
        assert!((sf.pedals[1] - 1.0).abs() < 0.01);
        assert!((sf.pedals[2] - 1.0).abs() < 0.01);
        for i in 0..5 {
            assert_eq!(sf.knee_levers[i], 0.0);
        }
        assert!((sf.volume - 0.5).abs() < 0.01);
        let expected = (3000.0 - 200.0) / 3600.0;
        for i in 0..4 {
            assert!((sf.bar_sensors[i] - expected).abs() < 0.01);
        }
        assert_eq!(sf.string_active, [false; 10]);
    }
}
