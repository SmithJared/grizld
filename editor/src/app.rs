use egui::Key;
use std::sync::Arc;
use vp_core::types::PlaybackState;

use crate::audio::{AudioOutput, SharedAudioState};
use crate::command::{parse_command, Command, CommandExecutor};
use crate::renderer::VideoRenderer;

#[derive(Clone)]
struct CommandSuggestion {
    command: String,
    description: String,
    example: String,
}

/// Main editor application
pub struct EditorApp {
    executor: CommandExecutor,
    renderer: VideoRenderer,

    // Audio output (must be kept alive)
    audio_output: Option<AudioOutput>,
    audio_state: SharedAudioState,

    // Command mode state
    command_mode: bool,
    command_input: String,
    command_history: Vec<String>,
    history_index: Option<usize>,
    command_suggestions: Vec<CommandSuggestion>,

    // Status messages
    status_message: String,
    error_message: Option<String>,

    // UI state
    should_quit: bool,
}

impl EditorApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let command_suggestions = vec![
            CommandSuggestion {
                command: "open".to_string(),
                description: "Open video file (shows file picker)".to_string(),
                example: ":open".to_string(),
            },
            CommandSuggestion {
                command: "open <path>".to_string(),
                description: "Open video from path".to_string(),
                example: ":open /path/to/video.mp4".to_string(),
            },
            CommandSuggestion {
                command: "play".to_string(),
                description: "Start playback".to_string(),
                example: ":play".to_string(),
            },
            CommandSuggestion {
                command: "pause".to_string(),
                description: "Pause playback".to_string(),
                example: ":pause".to_string(),
            },
            CommandSuggestion {
                command: "seek <time>".to_string(),
                description: "Seek to time (seconds, +/-, or %)".to_string(),
                example: ":seek 10.5 or :seek +5 or :seek 50%".to_string(),
            },
            CommandSuggestion {
                command: "buffer <id>".to_string(),
                description: "Switch to buffer by ID".to_string(),
                example: ":buffer 1 or :b 1".to_string(),
            },
            CommandSuggestion {
                command: "bnext".to_string(),
                description: "Switch to next buffer".to_string(),
                example: ":bnext or :bn".to_string(),
            },
            CommandSuggestion {
                command: "bprev".to_string(),
                description: "Switch to previous buffer".to_string(),
                example: ":bprev or :bp".to_string(),
            },
            CommandSuggestion {
                command: "buffers".to_string(),
                description: "List all open buffers".to_string(),
                example: ":buffers or :ls".to_string(),
            },
            CommandSuggestion {
                command: "bdelete <id>".to_string(),
                description: "Close buffer by ID".to_string(),
                example: ":bdelete 1 or :bd 1".to_string(),
            },
            CommandSuggestion {
                command: "quit".to_string(),
                description: "Exit editor".to_string(),
                example: ":quit or :q".to_string(),
            },
        ];

        // Create shared audio state
        let audio_state = SharedAudioState::new();

        // Create executor and set audio state
        let mut executor = CommandExecutor::new();
        executor.buffer_manager_mut().set_audio_state(audio_state.clone());

        Self {
            executor,
            renderer: VideoRenderer::new(),
            audio_output: None,
            audio_state,
            command_mode: false,
            command_input: String::new(),
            command_history: Vec::new(),
            history_index: None,
            command_suggestions,
            status_message: "Ready. Press ':' to enter command mode.".to_string(),
            error_message: None,
            should_quit: false,
        }
    }

    fn handle_keyboard_input(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if !self.command_mode {
                // Normal mode
                if i.key_pressed(Key::Colon) {
                    self.enter_command_mode();
                } else if i.key_pressed(Key::Space) {
                    self.toggle_play_pause();
                } else if i.key_pressed(Key::H) {
                    self.seek_relative(-5.0);
                } else if i.key_pressed(Key::L) {
                    self.seek_relative(5.0);
                } else if i.key_pressed(Key::J) {
                    self.seek_relative(-1.0);
                } else if i.key_pressed(Key::K) {
                    self.seek_relative(1.0);
                }
            } else {
                // Command mode - handled by TextEdit widget
            }
        });
    }

    fn enter_command_mode(&mut self) {
        self.command_mode = true;
        self.command_input.clear();
        self.history_index = None;
        self.error_message = None;
    }

    fn exit_command_mode(&mut self) {
        self.command_mode = false;
        self.command_input.clear();
    }

    fn execute_command(&mut self) {
        let input = format!(":{}", self.command_input);

        match parse_command(&input) {
            Ok(cmd) => {
                match cmd {
                    Command::Quit => {
                        self.should_quit = true;
                        return;
                    }
                    Command::OpenDialog => {
                        // Show file picker dialog
                        self.exit_command_mode();
                        self.show_file_picker();
                        return;
                    }
                    Command::NoOp => {}
                    _ => {
                        let is_open_cmd = matches!(cmd, Command::Open(_));

                        // Add to history
                        if !self.command_input.is_empty() {
                            self.command_history.push(self.command_input.clone());
                        }

                        // Execute
                        match self.executor.execute(cmd) {
                            Ok(msg) => {
                                self.status_message = msg;
                                self.error_message = None;

                                // If we just opened a file, initialize audio
                                if is_open_cmd {
                                    self.initialize_audio();
                                }
                            }
                            Err(err) => {
                                self.error_message = Some(err);
                            }
                        }
                    }
                }
            }
            Err(err) => {
                self.error_message = Some(err);
            }
        }

        self.exit_command_mode();
    }

    fn show_file_picker(&mut self) {
        // Show file picker with video file filters
        let file = rfd::FileDialog::new()
            .add_filter("Video Files", &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v"])
            .add_filter("All Files", &["*"])
            .set_title("Open Video File")
            .pick_file();

        if let Some(path) = file {
            // Execute the open command with the selected file
            match self.executor.execute(Command::Open(path.clone())) {
                Ok(msg) => {
                    self.status_message = msg;
                    self.error_message = None;
                    self.initialize_audio();
                }
                Err(err) => {
                    self.error_message = Some(err);
                }
            }
        } else {
            self.status_message = "File selection cancelled".to_string();
        }
    }

    fn initialize_audio(&mut self) {
        // Only initialize audio output once
        if self.audio_output.is_none() {
            match AudioOutput::new(self.audio_state.clone()) {
                Ok(audio) => {
                    self.audio_output = Some(audio);
                    tracing::info!("Audio output initialized with shared state");

                    // Set the initial active buffer if one exists
                    if let Some(player) = self.executor.player() {
                        let audio_buffer = Arc::new(player.audio_buffer().clone());
                        let clock = Arc::new(player.clock().clone());
                        self.audio_state.set_active(audio_buffer, clock);
                        tracing::info!("Audio state set to initial buffer");
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to initialize audio: {}", e);
                    self.error_message = Some(format!("Audio init failed: {}", e));
                }
            }
        }
    }

    fn toggle_play_pause(&mut self) {
        if let Some(player) = self.executor.player_mut() {
            match player.state() {
                PlaybackState::Playing => {
                    player.pause();
                    self.status_message = "Paused".to_string();
                }
                PlaybackState::Paused | PlaybackState::Stopped => {
                    player.play();
                    self.status_message = "Playing".to_string();
                }
            }
        } else {
            self.error_message = Some("No file loaded".to_string());
        }
    }

    fn seek_relative(&mut self, offset: f64) {
        if let Some(player) = self.executor.player_mut() {
            let current = player.current_time();
            let target = (current + offset).max(0.0).min(player.duration());

            if let Err(e) = player.seek(target) {
                self.error_message = Some(format!("Seek failed: {}", e));
            } else {
                self.status_message = format!("Seeked to {:.1}s", target);
            }
        }
    }

    fn render_video_viewport(&mut self, ui: &mut egui::Ui) {
        if let Some(player) = self.executor.player() {
            // Time how long it takes to get the current frame
            let get_frame_start = std::time::Instant::now();
            let frame_opt = player.get_current_frame();
            let get_frame_time = get_frame_start.elapsed().as_secs_f64() * 1000.0;

            if let Some(frame) = frame_opt {
                // Time texture update
                let texture_start = std::time::Instant::now();
                self.renderer.update_texture(ui.ctx(), &frame);
                let texture_time = texture_start.elapsed().as_secs_f64() * 1000.0;

                // Time actual rendering
                let render_start = std::time::Instant::now();
                self.renderer.render(ui);
                let render_time = render_start.elapsed().as_secs_f64() * 1000.0;

                // Log every 30 frames
                static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let frame_num = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                if frame_num % 30 == 0 {
                    tracing::info!(
                        "RENDER: get_frame={:.2}ms | texture={:.2}ms | render={:.2}ms",
                        get_frame_time, texture_time, render_time
                    );
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Buffering...");
                });
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.heading("Grizld - Vim-based Video Editor");
                ui.add_space(20.0);
                ui.label("Press ':' and type 'open <file>' to load a video");
            });
        }
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(player) = self.executor.player() {
                let current = player.current_time();
                let duration = player.duration();
                let progress = (current / duration).clamp(0.0, 1.0);

                // Playback state
                let state_icon = match player.state() {
                    PlaybackState::Playing => "▶",
                    PlaybackState::Paused => "⏸",
                    PlaybackState::Stopped => "⏹",
                };

                ui.label(format!("{} {:02}:{:05.2} / {:02}:{:05.2}",
                    state_icon,
                    (current / 60.0) as u32,
                    current % 60.0,
                    (duration / 60.0) as u32,
                    duration % 60.0,
                ));

                // Progress bar
                let progress_bar = egui::ProgressBar::new(progress as f32)
                    .show_percentage()
                    .desired_width(200.0);
                ui.add(progress_bar);

                // Buffer stats
                let (frame_count, audio_duration) = player.buffer_stats();
                ui.label(format!("Buf: {}f / {:.1}s", frame_count, audio_duration));
            }
        });
    }

    fn render_command_line(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if !self.command_mode {
                // Display status or error
                if let Some(err) = &self.error_message {
                    ui.colored_label(egui::Color32::RED, format!("Error: {}", err));
                } else {
                    ui.label(&self.status_message);
                }
            }
        });
    }

    fn render_command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_mode {
            return;
        }

        // Show command palette as a centered popup
        egui::Window::new("Command Palette")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 100.0])
            .fixed_size([600.0, 300.0])
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Command input
                    let mut should_execute = false;
                    let mut should_cancel = false;

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(":")
                                .size(20.0)
                                .color(egui::Color32::from_rgb(100, 200, 255)),
                        );

                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.command_input)
                                .desired_width(ui.available_width())
                                .font(egui::FontId::monospace(18.0))
                                .hint_text("Type a command..."),
                        );

                        // Auto-focus
                        response.request_focus();

                        // Check for Enter key (either lost focus with Enter, or Enter pressed while focused)
                        if (response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                            || ui.input(|i| i.key_pressed(Key::Enter))
                        {
                            should_execute = true;
                        }

                        // Check for Escape
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            should_cancel = true;
                        }
                    });

                    // Execute command after the UI rendering to avoid borrow issues
                    if should_execute {
                        self.execute_command();
                    } else if should_cancel {
                        self.exit_command_mode();
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(5.0);

                    // Suggestions
                    ui.label(
                        egui::RichText::new("Available Commands")
                            .size(14.0)
                            .color(egui::Color32::GRAY),
                    );

                    ui.add_space(5.0);

                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            let input_lower = self.command_input.to_lowercase();

                            for suggestion in &self.command_suggestions {
                                // Filter suggestions based on input
                                if input_lower.is_empty()
                                    || suggestion.command.to_lowercase().contains(&input_lower)
                                    || suggestion.description.to_lowercase().contains(&input_lower)
                                {
                                    ui.group(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(&suggestion.command)
                                                    .size(14.0)
                                                    .color(egui::Color32::from_rgb(100, 200, 255))
                                                    .family(egui::FontFamily::Monospace),
                                            );
                                            ui.label(
                                                egui::RichText::new(&suggestion.description)
                                                    .size(12.0)
                                                    .color(egui::Color32::LIGHT_GRAY),
                                            );
                                        });

                                        ui.label(
                                            egui::RichText::new(format!("  Example: {}", suggestion.example))
                                                .size(11.0)
                                                .color(egui::Color32::DARK_GRAY)
                                                .italics(),
                                        );
                                    });

                                    ui.add_space(3.0);
                                }
                            }
                        });

                    ui.add_space(5.0);
                    ui.separator();

                    // Help text
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Enter").color(egui::Color32::LIGHT_GRAY));
                        ui.label(egui::RichText::new("execute").color(egui::Color32::DARK_GRAY));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Esc").color(egui::Color32::LIGHT_GRAY));
                        ui.label(egui::RichText::new("cancel").color(egui::Color32::DARK_GRAY));
                    });
                });
            });
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let frame_start = std::time::Instant::now();

        // Handle keyboard input
        let keyboard_start = std::time::Instant::now();
        self.handle_keyboard_input(ctx);
        let keyboard_time = keyboard_start.elapsed().as_secs_f64() * 1000.0;

        // Main panel
        let ui_start = std::time::Instant::now();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                // Video viewport (takes most of the space)
                let viewport_height = ui.available_height() - 60.0; // Reserve space for controls
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), viewport_height),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        self.render_video_viewport(ui);
                    },
                );

                ui.separator();

                // Status bar
                self.render_status_bar(ui);

                ui.separator();

                // Command line
                self.render_command_line(ui);
            });
        });
        let ui_time = ui_start.elapsed().as_secs_f64() * 1000.0;

        // Render command palette overlay
        let palette_start = std::time::Instant::now();
        self.render_command_palette(ctx);
        let palette_time = palette_start.elapsed().as_secs_f64() * 1000.0;

        // Total frame time
        let total_frame_time = frame_start.elapsed().as_secs_f64() * 1000.0;

        // Log frame timing every 30 frames
        static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let frame_num = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if frame_num % 30 == 0 {
            tracing::info!(
                "FRAME: total={:.2}ms | keyboard={:.2}ms | ui={:.2}ms | palette={:.2}ms | fps={:.1}",
                total_frame_time, keyboard_time, ui_time, palette_time, 1000.0 / total_frame_time
            );
        }

        // Request continuous repaints for video playback
        if self.executor.has_player() {
            ctx.request_repaint();
        }

        // Handle quit
        if self.should_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
