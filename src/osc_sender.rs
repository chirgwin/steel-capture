use crate::types::*;
use crossbeam_channel::Receiver;
use log::{debug, error, info};
use rosc::{OscMessage, OscPacket, OscType};
use std::net::UdpSocket;

pub struct OscSender {
    rx: Receiver<CaptureFrame>,
    target: String,
}

impl OscSender {
    pub fn new(rx: Receiver<CaptureFrame>, target: String) -> Self {
        Self { rx, target }
    }

    /// Run the OSC sender loop. Blocks the calling thread.
    pub fn run(&self) {
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to bind UDP socket: {}", e);
                return;
            }
        };
        info!("OSC sender → {}", self.target);

        for frame in self.rx.iter() {
            if let Err(e) = self.send_frame(&socket, &frame) {
                debug!("OSC send error: {}", e);
            }
        }
        info!("OSC sender shutting down");
    }

    fn send_frame(
        &self,
        socket: &UdpSocket,
        frame: &CaptureFrame,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Pedals
        for (i, &val) in frame.pedals.iter().enumerate() {
            let addr = format!("/steel/pedal/{}", ["a", "b", "c"][i]);
            self.send_float(socket, &addr, val)?;
        }

        // Knee levers
        for (i, &val) in frame.knee_levers.iter().enumerate() {
            let addr = format!("/steel/knee/{}", i);
            self.send_float(socket, &addr, val)?;
        }

        // Volume
        self.send_float(socket, "/steel/volume", frame.volume)?;

        // Bar position
        match frame.bar_position {
            Some(pos) => {
                self.send_float(socket, "/steel/bar/pos", pos)?;
                self.send_float(socket, "/steel/bar/confidence", frame.bar_confidence)?;
            }
            None => {
                self.send_float(socket, "/steel/bar/pos", -1.0)?;
                self.send_float(socket, "/steel/bar/confidence", 0.0)?;
            }
        }

        // Bar source: 0=none, 1=sensor, 2=audio, 3=fused
        let source_val = match frame.bar_source {
            BarSource::None => 0.0,
            BarSource::Sensor => 1.0,
            BarSource::Audio => 2.0,
            BarSource::Fused => 3.0,
        };
        self.send_float(socket, "/steel/bar/source", source_val)?;

        // Raw bar sensor readings (for calibration/diagnostics)
        for (i, &val) in frame.bar_sensors.iter().enumerate() {
            let addr = format!("/steel/bar/sensor/{}", i);
            self.send_float(socket, &addr, val)?;
        }

        // Per-string pitches
        for (i, &hz) in frame.string_pitches_hz.iter().enumerate() {
            let addr = format!("/steel/pitch/{}", i);
            self.send_float(socket, &addr, hz as f32)?;
        }

        Ok(())
    }

    fn send_float(
        &self,
        socket: &UdpSocket,
        addr: &str,
        val: f32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args: vec![OscType::Float(val)],
        });
        let buf = rosc::encoder::encode(&msg)?;
        socket.send_to(&buf, &self.target)?;
        Ok(())
    }
}
