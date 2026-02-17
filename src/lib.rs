pub mod bar_inference;
pub mod bar_sensor;
pub mod console_display;
pub mod coordinator;
pub mod copedant;
pub mod data_logger;
pub mod osc_sender;
pub mod simulator;
pub mod string_detector;
pub mod types;
pub mod ws_server;

#[cfg(feature = "hardware")]
pub mod serial_reader;

#[cfg(feature = "gui")]
pub mod webview_app;
