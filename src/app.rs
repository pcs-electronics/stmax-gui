use std::error::Error;

use eframe::egui::{self, Color32, RichText, TextEdit};
use tokio::runtime::{Builder, Runtime};

use crate::protocol::DeviceForm;
use crate::serial::{SerialController, SerialSnapshot};

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const STARTUP_WINDOW_CONTENT_BUFFER: f32 = 20.0;
const STARTUP_WINDOW_RESIZE_EPSILON: f32 = 4.0;
const STARTUP_WINDOW_MIN_WIDTH: f32 = 920.0;
const STARTUP_WINDOW_MIN_HEIGHT: f32 = 520.0;

pub struct TokioEguiApp {
    _runtime: Runtime,
    controller: SerialController,
    snapshot: SerialSnapshot,
    selected_port: String,
    form: DeviceForm,
    show_factory_reset_confirm: bool,
    startup_auto_connect_attempted: bool,
    startup_window_fit_complete: bool,
    last_port_scan_generation: u64,
    last_readback_generation: u64,
}

impl TokioEguiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> AppResult<Self> {
        configure_theme(&cc.egui_ctx);

        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("stmax-tokio")
            .enable_io()
            .enable_time()
            .build()?;

        let controller = SerialController::spawn(&runtime, cc.egui_ctx.clone());
        let snapshot = controller.snapshot();

        Ok(Self {
            _runtime: runtime,
            controller,
            snapshot,
            selected_port: String::new(),
            form: DeviceForm::default(),
            show_factory_reset_confirm: false,
            startup_auto_connect_attempted: false,
            startup_window_fit_complete: false,
            last_port_scan_generation: 0,
            last_readback_generation: 0,
        })
    }

    fn sync_snapshot(&mut self) {
        let previous_connected_port = self.snapshot.connected_port.clone();

        if let Some(snapshot) = self.controller.try_snapshot() {
            self.snapshot = snapshot;
        }

        if self.snapshot.port_scan_generation != self.last_port_scan_generation {
            self.last_port_scan_generation = self.snapshot.port_scan_generation;
            self.apply_default_port_selection();

            if !self.startup_auto_connect_attempted {
                self.startup_auto_connect_attempted = true;
                self.auto_connect_default_device_on_startup();
            }
        }

        if self.snapshot.readback_generation != self.last_readback_generation {
            self.last_readback_generation = self.snapshot.readback_generation;
            self.apply_device_readback();
        }

        if previous_connected_port != self.snapshot.connected_port
            && self.snapshot.connected_port.is_some()
            && !self.snapshot.busy
        {
            self.controller.read_config();
        }
    }

    fn apply_default_port_selection(&mut self) {
        if let Some(port) = self
            .snapshot
            .ports
            .iter()
            .find(|port| port.is_preferred_device)
        {
            self.selected_port = port.port_name.clone();
            return;
        }

        let selection_still_exists = self
            .snapshot
            .ports
            .iter()
            .any(|port| port.port_name == self.selected_port);
        if selection_still_exists {
            return;
        }

        if let Some(port) = self.snapshot.ports.first() {
            self.selected_port = port.port_name.clone();
        } else {
            self.selected_port.clear();
        }
    }

    fn auto_connect_default_device_on_startup(&mut self) {
        let should_connect = self
            .snapshot
            .ports
            .iter()
            .any(|port| port.is_preferred_device)
            || self.snapshot.ports.len() == 1;

        if should_connect
            && self.snapshot.connected_port.is_none()
            && !self.selected_port.is_empty()
        {
            self.connect();
        }
    }

    fn apply_device_readback(&mut self) {
        if let Some(form) = &self.snapshot.readback_form {
            self.form = form.clone();
        }
    }

    fn connect(&self) {
        let port_name = self.selected_port.trim();
        if !port_name.is_empty() {
            self.controller.connect(port_name.to_owned());
        }
    }

    fn render_connection_panel(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let is_connected = self.snapshot.connected_port.is_some();
        let controls_enabled = !self.snapshot.busy;
        let selected_port_text = selected_port_text(&self.snapshot, &self.selected_port);

        ui.group(|ui| {
            ui.heading("Connection");
            ui.add_space(6.0);

            ui.add_enabled_ui(controls_enabled && !is_connected, |ui| {
                egui::ComboBox::from_id_salt("port_select")
                    .width(ui.available_width())
                    .selected_text(selected_port_text)
                    .show_ui(ui, |ui| {
                        for port in &self.snapshot.ports {
                            let label = if port.summary.is_empty() {
                                port.port_name.clone()
                            } else {
                                format!("{}  {}", port.port_name, port.summary)
                            };

                            ui.selectable_value(
                                &mut self.selected_port,
                                port.port_name.clone(),
                                label,
                            );
                        }
                    });
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        controls_enabled && !is_connected,
                        egui::Button::new("Refresh ports"),
                    )
                    .clicked()
                {
                    self.controller.refresh_ports();
                }

                if ui
                    .add_enabled(
                        controls_enabled && !is_connected && !self.selected_port.is_empty(),
                        egui::Button::new("Connect"),
                    )
                    .clicked()
                {
                    self.connect();
                }

                if ui
                    .add_enabled(
                        controls_enabled && is_connected,
                        egui::Button::new("Disconnect"),
                    )
                    .clicked()
                {
                    self.controller.disconnect();
                }
            });

            ui.add_space(6.0);
            ui.label(format!("Status: {}", self.snapshot.connection_status));
            if let Some(summary) = &self.snapshot.connected_usb_summary {
                if !summary.is_empty() {
                    ui.small(summary);
                }
            }
        })
        .response
        .rect
    }

    fn render_transmitter_panel(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let total_width = ui.available_width();
        ui.group(|ui| {
            let inner_width = group_inner_width(ui, total_width);
            ui.set_min_width(inner_width);
            ui.set_width(inner_width);

            ui.heading("RF and Audio");
            ui.add_space(6.0);

            egui::Grid::new("rf_audio_grid")
                .num_columns(3)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Power");
                    ui.add_sized(
                        [80.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.power_percent),
                    );
                    ui.label("%");
                    ui.end_row();

                    ui.label("Frequency");
                    ui.add_sized(
                        [80.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.frequency_mhz),
                    );
                    ui.label("MHz");
                    ui.end_row();

                    ui.label("Alarm temp");
                    ui.add_sized(
                        [80.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.alarm_temp_c),
                    );
                    ui.label("C");
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.checkbox(&mut self.form.stereo_mode, "Stereo output");

            ui.horizontal(|ui| {
                ui.label("Audio input");
                egui::ComboBox::from_id_salt("audio_input")
                    .selected_text(if self.form.digital_audio_input {
                        "Digital"
                    } else {
                        "Analog"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form.digital_audio_input, false, "Analog");
                        ui.selectable_value(&mut self.form.digital_audio_input, true, "Digital");
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Audio gain");
                egui::ComboBox::from_id_salt("audio_gain")
                    .selected_text(match self.form.audio_gain {
                        0 => "0 - Low",
                        1 => "1 - Normal",
                        2 => "2 - High",
                        _ => "Invalid",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form.audio_gain, 0, "0 - Low");
                        ui.selectable_value(&mut self.form.audio_gain, 1, "1 - Normal");
                        ui.selectable_value(&mut self.form.audio_gain, 2, "2 - High");
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Preemphasis");
                egui::ComboBox::from_id_salt("preemphasis")
                    .selected_text(if self.form.preemphasis_50us {
                        "50 uS"
                    } else {
                        "75 uS"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form.preemphasis_50us, true, "50 uS");
                        ui.selectable_value(&mut self.form.preemphasis_50us, false, "75 uS");
                    });
            });
        })
        .response
        .rect
    }

    fn render_rds_panel(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let total_width = ui.available_width();
        ui.group(|ui| {
            let inner_width = group_inner_width(ui, total_width);
            ui.set_min_width(inner_width);
            ui.set_width(inner_width);

            ui.heading("RDS");
            ui.add_space(6.0);

            ui.checkbox(&mut self.form.rds_enabled, "Enable RDS");

            egui::Grid::new("rds_grid")
                .num_columns(3)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("PI");
                    ui.add_sized(
                        [96.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.rds_pi_hex),
                    );
                    ui.label("hex");
                    ui.end_row();

                    ui.label("ECC");
                    ui.add_sized(
                        [96.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.rds_ecc),
                    );
                    ui.label("0..255");
                    ui.end_row();

                    ui.label("DI");
                    ui.add_sized(
                        [96.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.rds_di),
                    );
                    ui.label("0..15");
                    ui.end_row();

                    ui.label("PTY");
                    ui.add_sized(
                        [96.0, ui.spacing().interact_size.y],
                        TextEdit::singleline(&mut self.form.rds_pty),
                    );
                    ui.label("0..31");
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.label("Program service");
            ui.add(TextEdit::singleline(&mut self.form.rds_ps).hint_text("Up to 8 bytes"));

            ui.add_space(6.0);
            ui.label("Radio text");
            ui.add(TextEdit::singleline(&mut self.form.rds_rt).hint_text("Up to 64 bytes"));

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.form.rds_tp, "TP");
                ui.checkbox(&mut self.form.rds_ta, "TA");
                ui.checkbox(&mut self.form.rds_ms, "MS");
            });

            ui.add_space(8.0);
            ui.label("Alternative frequencies");
            ui.add(
                TextEdit::multiline(&mut self.form.rds_afs)
                    .desired_rows(2)
                    .hint_text("Comma or space separated, e.g. 99.5, 101.2, 104.7"),
            );
        })
        .response
        .rect
    }

    fn render_actions_panel(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let is_connected = self.snapshot.connected_port.is_some();
        let actions_enabled = !self.snapshot.busy && is_connected;
        let total_width = ui.available_width();

        ui.group(|ui| {
            let inner_width = group_inner_width(ui, total_width);
            ui.set_min_width(inner_width);
            ui.set_width(inner_width);

            ui.heading("Actions");
            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(actions_enabled, egui::Button::new("Read"))
                    .clicked()
                {
                    self.controller.read_config();
                }

                if ui
                    .add_enabled(actions_enabled, egui::Button::new("Save"))
                    .clicked()
                {
                    self.controller.save_config(self.form.clone());
                }

                if ui
                    .add_enabled(
                        actions_enabled,
                        egui::Button::new("Set to factory defaults"),
                    )
                    .clicked()
                {
                    self.show_factory_reset_confirm = true;
                }
            });

            ui.add_space(6.0);
            ui.label(format!("Last event: {}", self.snapshot.last_event));

            if let Some(error) = &self.snapshot.last_error {
                ui.colored_label(
                    Color32::from_rgb(150, 33, 27),
                    RichText::new(error).strong(),
                );
            } else {
                ui.colored_label(
                    Color32::from_rgb(38, 108, 68),
                    RichText::new("No active error").strong(),
                );
            }
        })
        .response
        .rect
    }

    fn fit_startup_window(
        &mut self,
        ctx: &egui::Context,
        panel_left: f32,
        panel_top: f32,
        panel_right: f32,
        panel_bottom: f32,
        content_left: f32,
        content_top: f32,
        content_right: f32,
        content_bottom: f32,
    ) {
        if self.startup_window_fit_complete
            || panel_right <= panel_left
            || panel_bottom <= panel_top
            || !content_left.is_finite()
            || !content_top.is_finite()
        {
            return;
        }

        let horizontal_gap = (content_left - panel_left).max(0.0);
        let vertical_gap = (content_top - panel_top).max(0.0);
        let desired_width =
            ((content_right - content_left) + horizontal_gap * 2.0 + STARTUP_WINDOW_CONTENT_BUFFER)
                .max(STARTUP_WINDOW_MIN_WIDTH)
                .round();
        let desired_height =
            ((content_bottom - content_top) + vertical_gap * 2.0 + STARTUP_WINDOW_CONTENT_BUFFER)
                .max(STARTUP_WINDOW_MIN_HEIGHT)
                .round();

        let current_size = ctx.content_rect().size();
        if (desired_width - current_size.x).abs() <= STARTUP_WINDOW_RESIZE_EPSILON
            && (desired_height - current_size.y).abs() <= STARTUP_WINDOW_RESIZE_EPSILON
        {
            self.startup_window_fit_complete = true;
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            desired_width,
            desired_height,
        )));
    }
}

impl eframe::App for TokioEguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_snapshot();
        let mut panel_left: f32 = 0.0;
        let mut panel_top: f32 = 0.0;
        let mut panel_right: f32 = 0.0;
        let mut panel_bottom: f32 = 0.0;
        let mut content_left: f32 = f32::INFINITY;
        let mut content_top: f32 = f32::INFINITY;
        let mut content_right: f32 = 0.0;
        let mut content_bottom: f32 = 0.0;

        egui::CentralPanel::default().show(ctx, |ui| {
            panel_left = ui.max_rect().left();
            panel_top = ui.max_rect().top();
            panel_right = ui.max_rect().right();
            panel_bottom = ui.max_rect().bottom();

            egui::ScrollArea::vertical().show(ui, |ui| {
                let connection_rect = self.render_connection_panel(ui);
                content_left = content_left.min(connection_rect.left());
                content_top = content_top.min(connection_rect.top());
                content_right = content_right.max(connection_rect.right());
                content_bottom = content_bottom.max(connection_rect.bottom());
                ui.add_space(10.0);

                ui.columns(2, |columns| {
                    let transmitter_rect = self.render_transmitter_panel(&mut columns[0]);
                    content_left = content_left.min(transmitter_rect.left());
                    content_top = content_top.min(transmitter_rect.top());
                    content_right = content_right.max(transmitter_rect.right());
                    content_bottom = content_bottom.max(transmitter_rect.bottom());

                    columns[0].add_space(10.0);
                    let actions_rect = self.render_actions_panel(&mut columns[0]);
                    content_left = content_left.min(actions_rect.left());
                    content_top = content_top.min(actions_rect.top());
                    content_right = content_right.max(actions_rect.right());
                    content_bottom = content_bottom.max(actions_rect.bottom());

                    let rds_rect = self.render_rds_panel(&mut columns[1]);
                    content_left = content_left.min(rds_rect.left());
                    content_top = content_top.min(rds_rect.top());
                    content_right = content_right.max(rds_rect.right());
                    content_bottom = content_bottom.max(rds_rect.bottom());
                });
            });
        });

        self.fit_startup_window(
            ctx,
            panel_left,
            panel_top,
            panel_right,
            panel_bottom,
            content_left,
            content_top,
            content_right,
            content_bottom,
        );

        if self.show_factory_reset_confirm {
            let mut keep_open = self.show_factory_reset_confirm;
            let mut confirm_reset = false;
            let mut close_requested = false;

            egui::Window::new("Confirm reset")
                .collapsible(false)
                .resizable(false)
                .fixed_size(egui::vec2(320.0, 110.0))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut keep_open)
                .show(ctx, |ui| {
                    ui.label("Send `config-defaults` to the transmitter and reload the form?");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close_requested = true;
                        }

                        if ui
                            .add_enabled(
                                self.snapshot.connected_port.is_some() && !self.snapshot.busy,
                                egui::Button::new("Confirm reset"),
                            )
                            .clicked()
                        {
                            confirm_reset = true;
                        }
                    });
                });

            if close_requested || confirm_reset {
                keep_open = false;
            }

            self.show_factory_reset_confirm = keep_open;

            if confirm_reset {
                self.controller.factory_defaults();
            }
        }
    }
}

fn selected_port_text(snapshot: &SerialSnapshot, selected_port: &str) -> String {
    snapshot
        .ports
        .iter()
        .find(|port| port.port_name == selected_port)
        .map(|port| port.port_name.clone())
        .unwrap_or_else(|| {
            if snapshot.ports.is_empty() {
                "No ports detected".to_owned()
            } else {
                selected_port.to_owned()
            }
        })
}

fn group_inner_width(ui: &egui::Ui, total_width: f32) -> f32 {
    let margin = egui::Frame::group(ui.style()).total_margin().sum().x;
    (total_width - margin).max(0.0)
}

fn configure_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = Color32::from_rgb(246, 242, 233);
    visuals.extreme_bg_color = Color32::from_rgb(235, 228, 214);
    visuals.faint_bg_color = Color32::from_rgb(240, 235, 226);
    visuals.selection.bg_fill = Color32::from_rgb(185, 97, 42);
    visuals.selection.stroke.color = Color32::from_rgb(251, 247, 241);
    visuals.widgets.active.bg_fill = Color32::from_rgb(185, 97, 42);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(217, 138, 86);
    visuals.widgets.open.bg_fill = Color32::from_rgb(227, 214, 196);
    visuals.window_fill = Color32::from_rgb(252, 249, 242);
    visuals.hyperlink_color = Color32::from_rgb(38, 94, 140);

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);

    ctx.set_style(style);
}
