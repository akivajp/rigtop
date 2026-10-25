//! A window for naming and hiding sensor channels (temperatures and fans),
//! saved to `[sensors]` in the configuration file.

use std::path::Path;

use egui::{RichText, Ui};
use rigtop_core::Snapshot;
use rigtop_core::config::SensorConfig;

use crate::widgets::{BAD, DIM, OK};

/// State of the sensors window: the names and hidden sensors being edited.
#[derive(Default)]
pub struct SensorsDialog {
    open: bool,
    draft: SensorConfig,
    /// Outcome of the last save (message, success).
    message: Option<(String, bool)>,
}

impl SensorsDialog {
    /// Opens the window on the current settings.
    pub fn open(&mut self, current: &SensorConfig) {
        self.open = true;
        self.draft = current.clone();
        self.message = None;
    }

    /// Shows the outcome of saving to the user.
    pub fn report(&mut self, message: String, ok: bool) {
        self.message = Some((message, ok));
    }
}

/// One sensor channel in the list.
struct Channel<'a> {
    key: &'a str,
    /// Name from the driver (or a generic one such as "fan2").
    label: &'a str,
    reading: String,
}

/// A heading (chip name, and the device that tells chips of the same kind
/// apart) with its channels.
type Group<'a> = (&'a str, &'a str, Vec<Channel<'a>>);

/// Channels grouped by chip. Channels without a key cannot be named and are
/// left out.
fn groups(snapshot: &Snapshot) -> Vec<Group<'_>> {
    let mut groups = Vec::new();
    // CPU readings that do not come from a listed chip (LibreHardwareMonitor).
    let in_chips = |key: &str| {
        snapshot
            .sensors
            .iter()
            .flat_map(|c| &c.temperatures)
            .any(|t| t.key == key)
    };
    let cpu: Vec<Channel> = snapshot
        .cpu
        .temperatures
        .iter()
        .filter(|t| !t.key.is_empty() && !in_chips(&t.key))
        .map(|t| Channel {
            key: &t.key,
            label: &t.label,
            reading: format!("{:.0}°C", t.celsius),
        })
        .collect();
    if !cpu.is_empty() {
        groups.push(("CPU", "", cpu));
    }
    for chip in &snapshot.sensors {
        let temperatures = chip.temperatures.iter().map(|t| Channel {
            key: &t.key,
            label: &t.label,
            reading: format!("{:.0}°C", t.celsius),
        });
        let fans = chip.fans.iter().map(|f| Channel {
            key: &f.key,
            label: &f.label,
            reading: format!("{} rpm", f.rpm),
        });
        let channels: Vec<Channel> = temperatures
            .chain(fans)
            .filter(|c| !c.key.is_empty())
            .collect();
        if !channels.is_empty() {
            // Chip keys are "<name>:<device>".
            let device = chip.key.split_once(':').map_or("", |k| k.1);
            groups.push((chip.name.as_str(), device, channels));
        }
    }
    groups
}

/// Whether `snapshot` has any sensor the window could name.
pub fn has_sensors(snapshot: &Snapshot) -> bool {
    !groups(snapshot).is_empty()
}

/// Draws the window for the sensors in `snapshot`. Returns the new settings
/// when the user saved; the caller writes them to the configuration file.
pub fn show(
    ctx: &egui::Context,
    dialog: &mut SensorsDialog,
    snapshot: &Snapshot,
    config_path: Option<&Path>,
) -> Option<SensorConfig> {
    if !dialog.open {
        return None;
    }
    let mut open = true;
    let mut result = None;
    let screen = ctx.content_rect();
    egui::Window::new("Sensors")
        .open(&mut open)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(egui::pos2(screen.center().x, screen.top() + 60.0))
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            result = contents(ui, dialog, snapshot, config_path, screen.height());
        });
    if !open {
        dialog.open = false;
    }
    result
}

fn contents(
    ui: &mut Ui,
    dialog: &mut SensorsDialog,
    snapshot: &Snapshot,
    config_path: Option<&Path>,
    screen_height: f32,
) -> Option<SensorConfig> {
    ui.label(
        RichText::new(
            "Named temperatures show in the CPU panel, next to the CPU's own; fans show \
             while they spin or once named. Names follow the sensor keys, so they also \
             apply to hosts with the same hardware.",
        )
        .color(DIM),
    );
    ui.add_space(6.0);
    let draft = &mut dialog.draft;
    egui::ScrollArea::vertical()
        .max_height((screen_height * 0.6).max(200.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (index, (name, device, channels)) in groups(snapshot).into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).strong());
                    ui.label(RichText::new(device).color(DIM));
                });
                egui::Grid::new(("sensors", index))
                    .striped(true)
                    .num_columns(4)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for channel in channels {
                            channel_row(ui, draft, &channel);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }
        });
    ui.separator();
    let mut result = None;
    ui.horizontal(|ui| {
        if ui.button("save").clicked() {
            result = Some(normalized(draft));
        }
        let target = config_path.map_or_else(
            || "no configuration file".to_owned(),
            |p| p.display().to_string(),
        );
        ui.label(RichText::new(target).color(DIM));
    });
    if let Some((message, ok)) = &dialog.message {
        ui.label(RichText::new(message).color(if *ok { OK } else { BAD }));
    }
    result
}

/// The driver's name, the reading, a field for the user's name and whether
/// the channel is shown.
fn channel_row(ui: &mut Ui, draft: &mut SensorConfig, channel: &Channel<'_>) {
    let label = ui.label(channel.label).on_hover_text(channel.key);
    ui.label(RichText::new(&channel.reading).color(DIM));
    let mut name = draft.labels.get(channel.key).cloned().unwrap_or_default();
    let field = egui::TextEdit::singleline(&mut name).hint_text(channel.label);
    let height = ui.spacing().interact_size.y;
    if ui
        .add_sized([180.0, height], field)
        .labelled_by(label.id)
        .changed()
    {
        // Kept as typed (spaces included) until saved.
        if name.is_empty() {
            draft.labels.remove(channel.key);
        } else {
            draft.labels.insert(channel.key.to_owned(), name);
        }
    }
    let mut shown = !draft.hidden.iter().any(|k| k == channel.key);
    if ui.checkbox(&mut shown, "show").changed() {
        if shown {
            draft.hidden.retain(|k| k != channel.key);
        } else {
            draft.hidden.push(channel.key.to_owned());
        }
    }
}

/// Names trimmed, empty ones dropped.
fn normalized(draft: &SensorConfig) -> SensorConfig {
    SensorConfig {
        labels: draft
            .labels
            .iter()
            .map(|(key, name)| (key.clone(), name.trim().to_owned()))
            .filter(|(_, name)| !name.is_empty())
            .collect(),
        hidden: draft.hidden.clone(),
    }
}
