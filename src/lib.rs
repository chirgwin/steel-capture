pub mod bar_inference;
pub mod bar_sensor;
pub mod calibration;
pub mod console_display;
pub mod coordinator;
pub mod copedant;
pub mod data_logger;
pub mod dsp;
pub mod jsonl_reader;
pub mod osc_sender;
pub mod simulator;
pub mod string_detector;
pub mod types;
pub mod wav_player;
pub mod ws_server;

#[cfg(feature = "calibration")]
pub mod audio_input;

#[cfg(feature = "calibration")]
pub mod calibrator;

#[cfg(feature = "hardware")]
pub mod serial_reader;

#[cfg(feature = "gui")]
pub mod webview_app;
