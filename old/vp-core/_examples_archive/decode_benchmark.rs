//! Benchmark for measuring FFmpeg decoder performance with multi-threading
//!
//! This example decodes an entire video file and measures:
//! - Pure decode time (hardware or software)
//! - Frames per second (FPS)
//! - Hardware vs software frame counts
//!
//! Usage:
//!   cargo run --example decode_benchmark --release -- <video_file> [--software]
//!
//! Example:
//!   cargo run --example decode_benchmark --release -- tests/resources/test_video.mp4
//!   cargo run --example decode_benchmark --release -- tests/resources/test_video.mp4 --software

use vp_core::{demux::Demuxer, decode::DecoderPreference, decode::create_decoder};
use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)  // Enable debug to see extraction details
        .init();
    
    // Get video file from command line
    let args: Vec<String> = env::args().collect();
    let video_file = if args.len() > 1 && !args[1].starts_with("--") {
        &args[1]
    } else {
        "tests/resources/test_video.mp4"
    };
    
    // Check for --software flag
    let use_software = args.iter().any(|arg| arg == "--software" || arg == "-s");
    let decoder_pref = if use_software {
        println!("Using SOFTWARE decoder");
        DecoderPreference::Software
    } else {
        println!("Using HARDWARE decoder (if available)");
        DecoderPreference::Hardware
    };
    
    println!("=== FFmpeg Decoder Benchmark ===");
    println!("Video file: {}", video_file);
    println!();
    
    // Create demuxer
    let mut demuxer = Demuxer::new(video_file)?;
    
    // Get video stream info
    let stream_info = demuxer.video_stream_info()
        .ok_or("No video stream found")?;
    
    println!("Video info:");
    println!("  Resolution: {}x{}", stream_info.width, stream_info.height);
    println!("  FPS: {:.2}", stream_info.fps);
    println!("  Duration: {:.2}s", stream_info.duration);
    println!("  Codec: {}", stream_info.codec_name);
    println!();
    
    // Create decoder
    let mut decoder = create_decoder(&stream_info, decoder_pref)?;
    
    println!("Decoder: {}", decoder.codec_name());
    println!("Hardware: {}", decoder.is_hardware());
    println!();
    
    // Benchmark decoding
    println!("Starting decode benchmark...");
    println!("(Measuring PURE decode time, no frame extraction/conversion)");
    println!();
    let start_time = Instant::now();
    
    let mut frame_count = 0;
    let mut hardware_frame_count = 0;
    let mut software_frame_count = 0;
    
    // Decode all frames (but don't extract/convert them)
    loop {
        // Read packet
        let packet = match demuxer.next_packet()? {
            Some(p) => p,
            None => break, // End of stream
        };
        
        // Decode packet (this is the actual decode operation)
        if let Some(frame) = decoder.decode_packet(&packet)? {
            frame_count += 1;
            
            // Track frame type (but don't convert!)
            match frame {
                vp_core::decode::VideoFrame::Hardware(_) => hardware_frame_count += 1,
                vp_core::decode::VideoFrame::Software(_) => software_frame_count += 1,
            }
            
            // Print progress every 100 frames
            if frame_count % 100 == 0 {
                print!("\rDecoded {} frames...", frame_count);
                use std::io::Write;
                std::io::stdout().flush().unwrap();
            }
        }
    }
    
    // Flush decoder to get remaining frames
    decoder.flush()?;
    
    let total_time = start_time.elapsed();
    
    println!("\r                                    "); // Clear progress line
    println!();
    println!("=== Benchmark Results ===");
    println!("Total frames decoded: {}", frame_count);
    println!("  Hardware frames: {} ({:.1}%)", 
             hardware_frame_count, 
             100.0 * hardware_frame_count as f64 / frame_count as f64);
    println!("  Software frames: {} ({:.1}%)", 
             software_frame_count,
             100.0 * software_frame_count as f64 / frame_count as f64);
    println!();
    println!("Total time: {:.3}s", total_time.as_secs_f64());
    println!("Decode FPS: {:.2}", frame_count as f64 / total_time.as_secs_f64());
    println!("Average time per frame: {:.2}ms", total_time.as_secs_f64() * 1000.0 / frame_count as f64);
    println!();
    
    // Calculate speedup vs real-time
    let video_duration = stream_info.duration;
    let speedup = video_duration / total_time.as_secs_f64();
    println!("Speedup vs real-time: {:.2}x", speedup);
    
    if speedup >= 1.0 {
        println!("✓ Decoder is fast enough for real-time playback!");
    } else {
        println!("⚠ Decoder is slower than real-time (may cause buffering)");
    }
    
    println!();
    println!("=== Performance Analysis ===");
    
    // Estimate buffer fill time
    let buffer_duration = 2.0; // Default max buffer duration
    let frames_for_buffer = (buffer_duration * stream_info.fps) as usize;
    let time_to_fill_buffer = total_time.as_secs_f64() * frames_for_buffer as f64 / frame_count as f64;
    
    println!("Estimated time to fill 2s buffer: {:.3}s", time_to_fill_buffer);
    
    if time_to_fill_buffer < 0.5 {
        println!("✓ Buffer fills very quickly (< 0.5s)");
    } else if time_to_fill_buffer < 1.0 {
        println!("✓ Buffer fills reasonably fast (< 1s)");
    } else {
        println!("⚠ Buffer fills slowly (> 1s) - may need optimization");
    }
    
    println!();
    
    if hardware_frame_count > 0 {
        println!("=== Hardware Decoding Notes ===");
        println!("Hardware frames are decoded on GPU and kept as IOSurface references.");
        println!("This benchmark measures PURE decode time without frame extraction.");
        println!();
        println!("Note: If you see slow performance with hardware frames, it means:");
        println!("  1. The frames ARE being decoded quickly on GPU");
        println!("  2. But they're being converted to CPU (YUV) format somewhere");
        println!("  3. That conversion is the bottleneck, not the decode itself");
        println!();
        println!("To get true hardware performance benefits:");
        println!("  - Use hardware frames directly (VideoFrame::Hardware)");
        println!("  - Render from IOSurface without CPU conversion");
        println!("  - Avoid calling methods that extract YUV data");
    } else {
        println!("=== Software Decoding Notes ===");
        println!("Software decoder uses CPU with FFmpeg multi-threading.");
        println!("Thread types enabled: FRAME + SLICE");
        println!();
        println!("To compare with hardware decoding:");
        println!("  cargo run --example decode_benchmark --release");
    }
    
    Ok(())
}
