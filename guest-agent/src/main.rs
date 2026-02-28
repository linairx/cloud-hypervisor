// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Guest Agent CLI entry point
//!
//! Runs inside the VM to capture frames, cursor, and audio.

use std::io;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clap::{Arg, Command};
use log::{error, info, LevelFilter};
use simple_logger::SimpleLogger;

use lg_guest_agent::{AgentConfig, GuestAgent};

fn main() -> io::Result<()> {
    // Parse command line arguments
    let matches = Command::new("lg-guest-agent")
        .version("0.1.0")
        .author("lg-capture Authors")
        .about("Guest Agent for lg-capture VM frame capture")
        .arg(
            Arg::new("shm-path")
                .short('s')
                .long("shm-path")
                .value_name("PATH")
                .default_value("/dev/shm/lg-capture")
                .help("Path to shared memory device"),
        )
        .arg(
            Arg::new("fps")
                .short('f')
                .long("fps")
                .value_name("FPS")
                .default_value("60")
                .help("Target frames per second"),
        )
        .arg(
            Arg::new("audio")
                .short('a')
                .long("audio")
                .action(clap::ArgAction::SetTrue)
                .help("Enable audio capture"),
        )
        .arg(
            Arg::new("sample-rate")
                .long("sample-rate")
                .value_name("HZ")
                .default_value("48000")
                .help("Audio sample rate in Hz"),
        )
        .arg(
            Arg::new("channels")
                .long("channels")
                .value_name("N")
                .default_value("2")
                .help("Number of audio channels"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(clap::ArgAction::SetTrue)
                .help("Enable verbose logging"),
        )
        .get_matches();

    // Initialize logging
    let log_level = if matches.get_flag("verbose") {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    SimpleLogger::new()
        .with_level(log_level)
        .init()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    // Build configuration
    let fps: u32 = matches
        .get_one::<String>("fps")
        .unwrap()
        .parse()
        .map_err(|e: std::num::ParseIntError| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

    let sample_rate: u32 = matches
        .get_one::<String>("sample-rate")
        .unwrap()
        .parse()
        .map_err(|e: std::num::ParseIntError| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

    let channels: u8 = matches
        .get_one::<String>("channels")
        .unwrap()
        .parse()
        .map_err(|e: std::num::ParseIntError| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

    let config = AgentConfig {
        shm_path: matches.get_one::<String>("shm-path").unwrap().clone(),
        target_fps: fps,
        enable_audio: matches.get_flag("audio"),
        audio_config: lg_guest_agent::AudioConfig {
            sample_rate,
            channels,
            ..Default::default()
        },
    };

    info!("Starting lg-guest-agent with config: {:?}", config);

    // Create guest agent
    let mut agent = GuestAgent::new(config)?;

    // Start capture
    agent.start()?;

    // Main loop
    let frame_interval = Duration::from_micros(1_000_000 / fps as u64);
    let mut last_frame = std::time::Instant::now();

    info!("Guest agent running, press Ctrl+C to stop");

    // Set up Ctrl+C handler
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    while running.load(Ordering::SeqCst) {
        // Run capture iteration
        if let Err(e) = agent.run_iteration() {
            error!("Capture iteration failed: {}", e);
        }

        // Maintain frame rate
        let elapsed = last_frame.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
        last_frame = std::time::Instant::now();
    }

    // Stop agent
    info!("Stopping guest agent...");
    agent.stop();

    info!("Guest agent stopped");
    Ok(())
}
