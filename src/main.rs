//! Compeller REACT - Native Audio-Reactive Visuals
//!
//! A native Rust application using egui/eframe for UI and OpenGL for rendering
//! projectM visualizations and video overlays with full GPU performance.

mod app;
mod config;
mod resources;
mod render;
mod audio;
mod video;
mod ui;
mod db;
mod track;
mod output;
mod sync;
mod compel_sync;
mod set_sync;
mod compel_matcher;
mod compel_picker;
mod midi;
mod emerge;
mod osc;
mod lighting;
mod hue;
mod wled;
mod dmx;
mod stage_sync;
mod stage_sync_worker;
mod thread_util;
mod camera;
mod auth;
mod youtube;

use eframe::egui;
use fs2::FileExt;
use std::fs::File;

/// Enforce single instance using a file lock
/// Returns the lock file handle if successful (must be kept alive), None if another instance is running
fn enforce_single_instance() -> Option<File> {
    // Get the config directory for the lock file
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("compeller-react");

    // Create config directory if it doesn't exist
    if let Err(e) = std::fs::create_dir_all(&config_dir) {
        log::warn!("Failed to create config directory: {}", e);
        // Continue anyway - we'll try to create the lock file
    }

    let lock_path = config_dir.join(".instance.lock");

    // Open or create the lock file
    let file = match File::create(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("Failed to create lock file: {}", e);
            return Some(File::open("/dev/null").ok()?); // Allow running if we can't create lock
        }
    };

    // Try to acquire exclusive lock (non-blocking)
    match file.try_lock_exclusive() {
        Ok(()) => {
            log::info!("Single instance lock acquired");
            Some(file)
        }
        Err(_) => {
            log::warn!("Another instance is already running");
            None
        }
    }
}

/// Load app icon from file (searches relative to executable and working directory)
fn load_icon() -> Option<egui::IconData> {
    let mut icon_paths = resources::candidates("public/images/compeller_icon_square.png");

    // Legacy bundle layout: icon directly under Contents/Resources
    if let Ok(exe) = std::env::current_exe() {
        let exe_resolved = exe.canonicalize().unwrap_or(exe);
        if let Some(exe_dir) = exe_resolved.parent() {
            icon_paths.push(exe_dir.join("../Resources/compeller_icon_square.png"));
        }
    }

    for path in &icon_paths {
        if let Ok(image_data) = std::fs::read(path) {
            if let Ok(image) = image::load_from_memory(&image_data) {
                let rgba = image.to_rgba8();
                let (width, height) = rgba.dimensions();
                return Some(egui::IconData {
                    rgba: rgba.into_raw(),
                    width,
                    height,
                });
            }
        }
    }
    log::warn!("Could not load app icon");
    None
}

fn main() -> eframe::Result<()> {
    // Set up GStreamer plugin path for bundled app
    // Plugins are copied to Contents/Resources/gstreamer-1.0/ by build.sh
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            let contents_dir = macos_dir.parent(); // Contents/
            let gst_path = contents_dir.map(|c| c.join("Resources").join("gstreamer-1.0"));
            if let Some(ref gst_dir) = gst_path {
                if gst_dir.is_dir() {
                    std::env::set_var("GST_PLUGIN_PATH", gst_dir);
                    std::env::set_var("GST_PLUGIN_SYSTEM_PATH", "");
                    // Force in-process plugin scanning — the forked gst-plugin-scanner
                    // binary can't resolve @loader_path rpaths in bundled dylibs
                    std::env::set_var("GST_REGISTRY_FORK", "no");
                    // Use app-local registry cache to avoid conflicts with system GStreamer.
                    // Always delete stale cache — in-process scan is fast (~100ms for 13 plugins)
                    // and prevents "LIVE but invisible" bugs from cached unloadable plugin entries.
                    if let Some(cache_dir) = dirs::cache_dir() {
                        let registry = cache_dir.join("compeller-react").join("gst-registry.bin");
                        if let Some(parent) = registry.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::remove_file(&registry);
                        std::env::set_var("GST_REGISTRY", &registry);
                    }
                    // GStreamer debug logging (bundled app only) — log warnings to file
                    // Override with GST_DEBUG env var for more verbose output
                    if std::env::var("GST_DEBUG").is_err() {
                        std::env::set_var("GST_DEBUG", "2");
                    }
                    if std::env::var("GST_DEBUG_FILE").is_err() {
                        if let Some(cache_dir) = dirs::cache_dir() {
                            let debug_log = cache_dir.join("compeller-react").join("gst-debug.log");
                            std::env::set_var("GST_DEBUG_FILE", &debug_log);
                        }
                    }
                }
            }
        }
    }

    // Initialize logging - OSC and Track matching at info level, others at error
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("error,compeller_react::osc=info,compeller_react::app=info,compeller_react::compel_picker=info,compeller_react::output=info,compeller_react::hue=info,compeller_react::wled=info")
    ).init();

    // Single-instance enforcement using file lock
    // The lock file handle must be kept alive for the duration of the app
    let _lock_file = match enforce_single_instance() {
        Some(f) => f,
        None => {
            eprintln!("REACT is already running. Only one instance allowed.");
            std::process::exit(1);
        }
    };

    // Load app icon
    let icon = load_icon();

    // Native options for the window
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("REACT by Compeller.ai")
        .with_inner_size([1600.0, 900.0])
        .with_min_inner_size([800.0, 600.0])
        .with_decorations(true)
        .with_transparent(false);

    // Set icon if loaded
    if let Some(icon_data) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon_data));
        log::info!("App icon loaded");
    }

    let native_options = eframe::NativeOptions {
        viewport,
        vsync: true,
        multisampling: 0,           // We handle our own AA in the render pipeline
        depth_buffer: 0,            // Not needed for 2D compositing
        stencil_buffer: 0,
        hardware_acceleration: eframe::HardwareAcceleration::Required,
        renderer: eframe::Renderer::Glow,  // OpenGL backend
        ..Default::default()
    };

    // Run the eframe application
    eframe::run_native(
        "Compeller",
        native_options,
        Box::new(|cc| Ok(Box::new(app::CompellerApp::new(cc)))),
    )
}
