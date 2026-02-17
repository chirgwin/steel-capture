use steel_capture::coordinator;
use steel_capture::copedant::buddy_emmons_e9;
use steel_capture::simulator;
use steel_capture::types::*;
#[cfg(feature = "gui")]
use steel_capture::webview_app;
#[cfg(feature = "hardware")]
use steel_capture::serial_reader;
use steel_capture::console_display;
use steel_capture::data_logger;
use steel_capture::osc_sender;
use steel_capture::ws_server;

use clap::Parser;
use crossbeam_channel::{bounded, unbounded};
use log::{error, info};
use std::path::PathBuf;
use std::thread;

#[derive(Parser)]
#[command(name = "steel-capture")]
#[command(about = "Pedal steel guitar expression capture system")]
struct Cli {
    /// Run in simulator mode (no hardware required)
    #[arg(long, default_value_t = true)]
    simulate: bool,

    /// Serial port for Teensy (e.g., /dev/ttyACM0)
    #[arg(long, default_value = "/dev/ttyACM0")]
    port: String,

    /// OSC target address
    #[arg(long, default_value = "127.0.0.1:9000")]
    osc_target: String,

    /// Enable OSC output
    #[arg(long)]
    osc: bool,

    /// Enable data logging
    #[arg(long)]
    log_data: bool,

    /// Output directory for logged sessions
    #[arg(long, default_value = "./sessions")]
    output_dir: PathBuf,

    /// Enable console display (terminal TUI, for headless/debug)
    #[arg(long)]
    console: bool,

    /// Console display refresh rate (Hz)
    #[arg(long, default_value_t = 20)]
    display_hz: u32,

    /// Disable the native GUI window
    #[arg(long)]
    no_gui: bool,

    /// Enable WebSocket server for browser visualization
    #[arg(long)]
    ws: bool,

    /// WebSocket server bind address
    #[arg(long, default_value = "0.0.0.0:8080")]
    ws_addr: String,

    /// WebSocket broadcast rate (Hz)
    #[arg(long, default_value_t = 60)]
    ws_fps: u32,

    /// Sensor sample rate (Hz)
    #[arg(long, default_value_t = 1000)]
    sensor_rate: u32,

    /// Simulator demo sequence: "basic" (default), "e9" (90s scripted tour), or "improv" (algorithmic)
    #[arg(long, default_value = "basic")]
    demo: String,

    /// Use audio-based string detection instead of simulator ground truth.
    /// Automatically enabled in hardware mode. Use with simulator to test
    /// the string detector against synthetic audio.
    #[arg(long)]
    detect_strings: bool,

    /// Suppress auto-opening the browser when --ws is active.
    #[arg(long)]
    no_open: bool,
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_millis()
    .init();

    let cli = Cli::parse();
    let copedant = buddy_emmons_e9();
    let clock = SessionClock::new();

    let gui_enabled = cfg!(feature = "gui") && !cli.no_gui;

    info!("═══════════════════════════════════════════════");
    info!("  STEEL CAPTURE v{}", env!("CARGO_PKG_VERSION"));
    info!("  Copedant: {}", copedant.name);
    info!("  Mode: {}", if cli.simulate { "SIMULATOR" } else { "HARDWARE" });
    if gui_enabled { info!("  UI: WebView (wry) → http://{}", cli.ws_addr.replace("0.0.0.0", "localhost")); }
    if cli.ws { info!("  UI: WebSocket on {}", cli.ws_addr); }
    if cli.console { info!("  UI: Console TUI"); }
    info!("═══════════════════════════════════════════════");

    // Channel: inputs → coordinator
    let (input_tx, input_rx) = bounded::<InputEvent>(4096);

    // Channels: coordinator → consumers
    let mut frame_txs: Vec<crossbeam_channel::Sender<CaptureFrame>> = Vec::new();

    // Audio logging channel
    let (audio_log_tx, audio_log_rx) = unbounded::<AudioChunk>();

    let mut handles = Vec::new();

    // ─── Console display (opt-in, for headless/debug) ───────────────
    if cli.console {
        let (tx, rx) = bounded::<CaptureFrame>(256);
        frame_txs.push(tx);
        let hz = cli.display_hz;
        handles.push(thread::Builder::new().name("display".into()).spawn(move || {
            console_display::ConsoleDisplay::new(rx, hz).run();
        }).unwrap());
    }

    // ─── OSC sender ─────────────────────────────────────────────────
    if cli.osc {
        let (tx, rx) = bounded::<CaptureFrame>(1024);
        frame_txs.push(tx);
        let target = cli.osc_target.clone();
        handles.push(thread::Builder::new().name("osc".into()).spawn(move || {
            osc_sender::OscSender::new(rx, target).run();
        }).unwrap());
    }

    // ─── Data logger ────────────────────────────────────────────────
    if cli.log_data {
        let (tx, rx) = bounded::<CaptureFrame>(4096);
        frame_txs.push(tx);
        let output_dir = cli.output_dir.clone();
        let cop = copedant.clone();
        handles.push(thread::Builder::new().name("logger".into()).spawn(move || {
            data_logger::DataLogger::new(rx, audio_log_rx, &output_dir, cop).run();
        }).unwrap());
    }

    // ─── WebSocket server ────────────────────────────────────────────
    // Always started when the webview GUI is active (it needs it to load the viz).
    // Also started when --ws is passed explicitly for external browser access.
    if gui_enabled || cli.ws {
        let (tx, rx) = bounded::<CaptureFrame>(1024);
        frame_txs.push(tx);
        let ws_addr = cli.ws_addr.clone();
        let ws_fps = cli.ws_fps;
        let viz_path = std::env::current_dir()
            .unwrap_or_default()
            .join("visualization.html");
        handles.push(thread::Builder::new().name("ws-server".into()).spawn(move || {
            ws_server::WsServer::new(rx, ws_addr, ws_fps, viz_path).run();
        }).unwrap());

        // Auto-open a browser tab when --ws is explicit and the webview is NOT running.
        // (When webview is running it IS the browser; no external tab needed.)
        if cli.ws && !gui_enabled && !cli.no_open {
            let url = format!("http://{}", cli.ws_addr.replace("0.0.0.0", "localhost"));
            handles.push(thread::Builder::new().name("browser-open".into()).spawn(move || {
                thread::sleep(std::time::Duration::from_millis(800));
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&url).spawn();
                #[cfg(target_os = "linux")]
                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                info!("Browser opened at {}", url);
            }).unwrap());
        }
    }

    // (No Rust channel needed for the webview — it reads frames via WebSocket.)

    // ─── Coordinator ────────────────────────────────────────────────
    let cop2 = copedant.clone();
    let audio_tx = if cli.log_data { Some(audio_log_tx) } else { None };
    let use_audio_detect = cli.detect_strings || !cli.simulate;
    handles.push(thread::Builder::new().name("coordinator".into()).spawn(move || {
        let mut coord = coordinator::Coordinator::new(input_rx, frame_txs, audio_tx, cop2)
            .with_audio_detection(use_audio_detect);
        coord.run();
    }).unwrap());

    // ─── Input source ───────────────────────────────────────────────
    if cli.simulate {
        info!("Starting simulator...");
        let sim_clock = clock.clone();
        let sim_tx = input_tx.clone();
        let sim_cop = copedant.clone();
        let rate = cli.sensor_rate;
        handles.push(thread::Builder::new().name("simulator".into()).spawn(move || {
            let demo = cli.demo.clone();
            simulator::Simulator::new(sim_clock, sim_tx, sim_cop, rate).run(&demo);
        }).unwrap());
    } else {
        #[cfg(feature = "hardware")]
        {
            info!("Starting serial reader on {}...", cli.port);
            let ser_clock = clock.clone();
            let ser_tx = input_tx.clone();
            let port = cli.port.clone();
            handles.push(thread::Builder::new().name("serial".into()).spawn(move || {
                serial_reader::SerialReader::new(port, ser_tx, ser_clock).run();
            }).unwrap());
        }
        #[cfg(not(feature = "hardware"))]
        {
            error!("Hardware mode requires 'hardware' feature. Falling back to simulator.");
            let sim_clock = clock.clone();
            let sim_tx = input_tx.clone();
            let sim_cop = copedant.clone();
            let rate = cli.sensor_rate;
            let demo = cli.demo.clone();
            handles.push(thread::Builder::new().name("simulator".into()).spawn(move || {
                simulator::Simulator::new(sim_clock, sim_tx, sim_cop, rate).run(&demo);
            }).unwrap());
        }
    }

    // ─── Launch WebView on main thread (blocks until window closes) ──
    //
    // WKWebView (via wry/tao) MUST run on the main thread on macOS.
    // All other threads are already spawned above.
    // webview_app::run() never returns — it exits the process on close.
    #[cfg(feature = "gui")]
    if gui_enabled {
        // Give the WS server a moment to bind before the WebView tries to load.
        thread::sleep(std::time::Duration::from_millis(600));
        let url = format!("http://{}", cli.ws_addr.replace("0.0.0.0", "localhost"));
        info!("Launching WebView at {}", url);
        webview_app::run(&url);
    }

    // If no GUI (--no-gui or feature disabled), wait for threads
    info!("Running headless. Press Ctrl+C to stop.");
    for h in handles {
        let _ = h.join();
    }
}
