  Current State Summary

  You're working on Grizld, a Vim-inspired video
  - vp-core: Video playback library (2,178 lines
  - editor: GUI application (2,465 lines)  Curre
  Critical Issues to Fix First

  1. Audio/Video Sync Bug (blocking issue)

  The current commit message indicates frames pl

  In vp-core/src/player.rs:232-262:
  - Frame display logic bypasses PTS checking du
  - get_latest() and get_first() fallbacks displ
  - Should only use fallbacks during active play

  Quick fixes needed:
  - Add state check in get_current_frame() to re
  - Align initial clock sync with audio buffer P
  - Review frame dropping threshold (0.1s may be

  2. Audio Buffer Overflow (CRITICAL comment in

  player.rs:417 notes audio buffer overflow brea

  High-Priority Cleanup

  Code Quality

  1. Remove excessive debug logging
    - Static atomics for log throttling scattere
    - Performance monitoring mixed with business
    - Suggestion: Use tracing spans with configu
  2. Remove commented code
    - video.rs:48-57 has commented scaler code
    - Dead code paths in flush methods
    - Action: Delete all commented code
  3. Improve error handling
    - Many .unwrap() calls in critical paths (au
    - Suggestion: Use proper Result propagation

  Project Structure

  1. Remove stub files
    - src/main.rs is just "Hello World" (unused)
    - Empty editor/src/ui/ directory
    - Action: Delete unused files
  2. Consolidate configuration
    - Two Cargo.toml files for vp-core
    - References to non-existent video-sys crate
    - Action: Clean up workspace configuration
  3. Address /old/ directory
    - Contains ~9,000 lines of previous implemen
    - Decision needed: Archive to git history or

  Medium-Priority Architectural Improvements

  1. Metal Rendering Cleanup

  // editor/src/metal/layer_manager.rs
  // Set layer opacity to 1.0 (currently 0.7 for
  Finalize the Metal integration and remove test

  2. Buffer System Refactoring

  Extract common patterns between FrameBuffer an
  - Both use Arc<Mutex<>> pattern
  - Similar overflow handling
  - Could share a trait for capacity management

  3. Separate Performance Monitoring

  Create a dedicated metrics module instead of i
  // vp-core/src/metrics.rs
  pub struct DecoderMetrics {
      frames_decoded: AtomicU64,
      // ... other metrics
  }

  4. Clock System Evolution

  Current Mutex-based clock is documented as "MV
  - Profile lock contention in CPAL callback
  - Consider if this is actually a bottleneck be

  Low-Priority Improvements

  Documentation

  - Add module-level docs to main components
  - Document hardware requirements (VideoToolbox
  - Update README to reflect current architectur
  - Consider removing or updating extensive /doc

  Testing

  - Currently only 2 test modules
  - Add unit tests for buffer management
  - Integration tests for A/V sync scenarios
  - Benchmarks for decode performance

  Code Style Consistency

  - Extract magic numbers (0.1s frame drop threshold, 15 frame capacity)
  - Consistent import grouping
  - Standardize error message formatting

  Recommended Action Plan

  1. Immediate (fix critical bugs):
    - Fix pause state frame display
    - Resolve audio buffer overflow handling
    - Test and verify A/V sync
  2. Short-term (cleanup):
    - Remove debug logging atomics
    - Delete commented code
    - Clean up project structure (stub files, old directory)
    - Set Metal layer opacity to 1.0
  3. Medium-term (refactor):
    - Extract performance monitoring
    - Improve error handling (replace unwraps)
    - Consolidate buffer patterns
  4. Long-term (architectural):
    - Lock-free clock (if profiling shows need)
    - Dual-buffer system for scrubbing
    - Linux hardware acceleration (VAAPI)
