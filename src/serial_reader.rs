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
/// | 6      | 2×9  | ADC values (u16 × 9 channels) |
/// | 24     | 2    | CRC16        |
/// | Total: 26 bytes              |
///
/// Channel order: A0=pedal_A, A1=pedal_B, A2=pedal_C,
///   A3=LKL, A4=LKR, A5=LKV, A6=RKL, A7=RKR, A8=volume
const FRAME_SIZE: usize = 26;
const SYNC_WORD: u16 = 0xBEEF;
const NUM_CHANNELS: usize = 9;

/// Calibration: maps raw ADC (0–4095 for Teensy's 12-bit ADC) to 0.0–1.0.
/// Each channel has min/max raw values.
#[derive(Clone)]
pub struct Calibration {
    /// (min_raw, max_raw) for each of 9 channels
    pub ranges: [(u16, u16); NUM_CHANNELS],
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            // Default: assume full ADC range. Real calibration should be done
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
    pub fn new(
        port_name: String,
        tx: Sender<InputEvent>,
        clock: SessionClock,
    ) -> Self {
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
        info!("Opening serial port: {} @ {}", self.port_name, self.baud_rate);

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

                            let frame_bytes: Vec<u8> =
                                frame_buf.drain(..FRAME_SIZE).collect();

                            match parse_frame(&frame_bytes, &self.calibration, &self.clock) {
                                Ok(sensor) => {
                                    let _ = self.tx.send(InputEvent::Sensor(sensor));
                                    frame_count += 1;
                                    if frame_count % 5000 == 0 {
                                        info!("Serial: {} frames, {} errors",
                                              frame_count, error_count);
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
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == 0xEF && buf[i + 1] == 0xBE {
            // Little-endian 0xBEEF
            return Some(i);
        }
    }
    None
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
    let sync = cursor.read_u16::<LittleEndian>().map_err(|e| e.to_string())?;
    if sync != SYNC_WORD {
        return Err(format!("bad sync: 0x{:04X}", sync));
    }

    // Timestamp from Teensy (u32 microseconds, wrapping)
    let _teensy_ts = cursor.read_u32::<LittleEndian>().map_err(|e| e.to_string())?;

    // ADC values (9 channels, u16 each)
    let mut raw = [0u16; NUM_CHANNELS];
    for i in 0..NUM_CHANNELS {
        raw[i] = cursor.read_u16::<LittleEndian>().map_err(|e| e.to_string())?;
    }

    // CRC16
    let received_crc = cursor.read_u16::<LittleEndian>().map_err(|e| e.to_string())?;
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
            calibrated[3], calibrated[4], calibrated[5],
            calibrated[6], calibrated[7],
        ],
        volume: calibrated[8],
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
}
