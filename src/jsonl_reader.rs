//! JSONL session reader — parses recorded sessions back into CaptureFrames.
//!
//! Reads the header line (format, copedant, channels) then yields frames
//! one at a time. Works with any `BufRead`: files, in-memory buffers, stdin.

use crate::types::{CaptureFrame, CompactFrame};
use std::io::BufRead;

/// Parsed JSONL header (first line of a session file).
#[derive(Debug)]
pub struct SessionHeader {
    pub format: String,
    pub rate_hz: u32,
    pub copedant_name: String,
    pub channels: Vec<serde_json::Value>,
    pub raw: serde_json::Value,
}

/// Line-by-line JSONL session reader.
pub struct SessionReader<R: BufRead> {
    reader: R,
    pub header: SessionHeader,
    line_buf: String,
}

impl<R: BufRead> SessionReader<R> {
    /// Read and validate the header line. Returns an error if the header
    /// is missing, unparseable, or lacks a `"format": "steel-capture"` field.
    pub fn open(mut reader: R) -> Result<Self, String> {
        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .map_err(|e| format!("read header: {}", e))?;

        let first_line = first_line.trim();
        if first_line.is_empty() {
            return Err("empty file".into());
        }

        let raw: serde_json::Value =
            serde_json::from_str(first_line).map_err(|e| format!("parse header: {}", e))?;

        let format = raw["format"]
            .as_str()
            .ok_or("missing \"format\" field")?
            .to_string();
        if format != "steel-capture" {
            return Err(format!("unknown format: {}", format));
        }

        let rate_hz = raw["rate_hz"].as_u64().unwrap_or(60) as u32;
        let copedant_name = raw["copedant"]["name"].as_str().unwrap_or("").to_string();
        let channels = raw["channels"].as_array().cloned().unwrap_or_default();

        Ok(Self {
            reader,
            header: SessionHeader {
                format,
                rate_hz,
                copedant_name,
                channels,
                raw,
            },
            line_buf: String::new(),
        })
    }

    /// Read the next frame. Returns `None` at EOF, `Err` for unparseable lines.
    pub fn next_frame(&mut self) -> Option<Result<CaptureFrame, String>> {
        self.line_buf.clear();
        match self.reader.read_line(&mut self.line_buf) {
            Ok(0) => None, // EOF
            Ok(_) => {
                let trimmed = self.line_buf.trim();
                if trimmed.is_empty() {
                    return self.next_frame(); // skip blank lines
                }
                Some(
                    serde_json::from_str::<CompactFrame>(trimmed)
                        .map(CaptureFrame::from)
                        .map_err(|e| format!("parse frame: {}", e)),
                )
            }
            Err(e) => Some(Err(format!("read line: {}", e))),
        }
    }

    /// Read all remaining frames, skipping malformed lines.
    pub fn read_all(mut self) -> Vec<CaptureFrame> {
        let mut frames = Vec::new();
        while let Some(result) = self.next_frame() {
            if let Ok(frame) = result {
                frames.push(frame);
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn minimal_header() -> String {
        r#"{"format":"steel-capture","rate_hz":60,"copedant":{"name":"Test"},"channels":[]}"#
            .to_string()
    }

    fn minimal_frame(ts: u64) -> String {
        serde_json::to_string(&CompactFrame {
            t: ts,
            p: [0.0; 3],
            kl: [0.0; 5],
            v: 0.7,
            bs: [0.0; 4],
            bp: None,
            bc: 0.0,
            bx: crate::types::BarSource::None,
            hz: [0.0; 10],
            sa: [false; 10],
            at: [false; 10],
            am: [0.0; 10],
        })
        .unwrap()
    }

    #[test]
    fn test_open_valid_header() {
        let data = minimal_header() + "\n";
        let reader = SessionReader::open(Cursor::new(data)).unwrap();
        assert_eq!(reader.header.format, "steel-capture");
        assert_eq!(reader.header.rate_hz, 60);
        assert_eq!(reader.header.copedant_name, "Test");
    }

    #[test]
    fn test_open_missing_format() {
        let data = r#"{"rate_hz":60}"#.to_string() + "\n";
        let result = SessionReader::open(Cursor::new(data));
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("format"), "got: {}", err);
    }

    #[test]
    fn test_open_wrong_format() {
        let data = r#"{"format":"something-else"}"#.to_string() + "\n";
        let result = SessionReader::open(Cursor::new(data));
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("unknown format"), "got: {}", err);
    }

    #[test]
    fn test_open_empty_file() {
        let result = SessionReader::open(Cursor::new(""));
        assert!(result.is_err());
    }

    #[test]
    fn test_read_frames() {
        let mut data = minimal_header() + "\n";
        data += &minimal_frame(1000);
        data += "\n";
        data += &minimal_frame(2000);
        data += "\n";

        let reader = SessionReader::open(Cursor::new(data)).unwrap();
        let frames = reader.read_all();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].timestamp_us, 1000);
        assert_eq!(frames[1].timestamp_us, 2000);
    }

    #[test]
    fn test_read_all_skips_malformed() {
        let mut data = minimal_header() + "\n";
        data += &minimal_frame(1000);
        data += "\n";
        data += "this is not json\n";
        data += &minimal_frame(3000);
        data += "\n";

        let reader = SessionReader::open(Cursor::new(data)).unwrap();
        let frames = reader.read_all();
        assert_eq!(frames.len(), 2, "should skip garbled line");
        assert_eq!(frames[0].timestamp_us, 1000);
        assert_eq!(frames[1].timestamp_us, 3000);
    }

    #[test]
    fn test_next_frame_reports_error() {
        let mut data = minimal_header() + "\n";
        data += "garbage\n";

        let mut reader = SessionReader::open(Cursor::new(data)).unwrap();
        let result = reader.next_frame().unwrap();
        assert!(result.is_err());
    }
}
