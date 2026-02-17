use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

/// Open a native WebView window (WKWebView on macOS) loading `url`.
/// Runs the tao event loop on the current thread and never returns
/// (exits the process when the window is closed).
pub fn run(url: &str) -> ! {
    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title("Steel Capture")
        .with_inner_size(tao::dpi::LogicalSize::new(1200_u32, 900_u32))
        .with_min_inner_size(tao::dpi::LogicalSize::new(800_u32, 600_u32))
        .build(&event_loop)
        .expect("Failed to create window");

    let _webview = WebViewBuilder::new()
        .with_url(url)
        .build(&window)
        .expect("Failed to create WebView");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    })
}
