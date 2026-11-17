//! The Session Window (decision 17): one small window per connected Controller, which the Agent opens as
//! `rcli session-window <who>`. It shows who it is, how long they have been connected and what they are doing, and
//! its controls act on that connection only. It closes itself when the connection ends.

use std::path::PathBuf;

use eframe::egui;
use rcli_agent::ipc::{self, Request};
use serde_json::{json, Value};

use super::look;

/// The `rcli` argument that opens a Session Window.
pub const COMMAND: &str = "session-window";

/// Opens the Session Window of the Controller `who` (its display name, as `rcli agent info` lists it).
pub fn run(who: String, socket: Option<PathBuf>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let options = eframe::NativeOptions {
        viewport: look::viewport().with_inner_size([400.0, 380.0]).with_min_inner_size([320.0, 240.0]),
        ..Default::default()
    };
    let socket = socket.unwrap_or_else(ipc::default_socket);
    eframe::run_native(
        &format!("rcli: {who}"),
        options,
        Box::new(move |cc| {
            look::install(&cc.egui_ctx);
            Ok(Box::new(Window { who, socket, rt, note: String::new() }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("cannot open the Session Window: {e}"))?;
    Ok(())
}

struct Window {
    who: String,
    socket: PathBuf,
    rt: tokio::runtime::Runtime,
    /// The last action's message.
    note: String,
}

impl Window {
    /// One IPC request; refused only for lack of root (Service Mode), `rcli <root>` runs it after the OS elevation
    /// prompt. An empty `root` never elevates. Accepting, granting root or switching a capability on shows the OS
    /// authentication dialog to the Device Owner (ADR 0013).
    fn send(&self, request: &Request, root: &[&str]) -> Value {
        let sent = ipc::send_authenticated(&self.socket, request, &ipc::OsAuth::Dialog);
        let reply = self.rt.block_on(sent).unwrap_or_else(super::error_reply);
        if root.is_empty() || !super::refused_for_root(&reply) {
            return reply;
        }
        match super::elevated(root, None) {
            Ok(out) => json!({ "message": out }),
            Err(e) => json!({ "error": e.to_string() }),
        }
    }

    fn act(&mut self, request: Request, root: &[&str]) {
        self.note = super::note(&self.send(&request, root));
    }
}

impl eframe::App for Window {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        let info = super::fetch(&self.rt, &self.socket);
        let connection = info["connections"].as_array().and_then(|c| c.iter().find(|c| c["name"] == self.who.as_str())).cloned();
        let who = self.who.clone();
        let Some(connection) = connection else {
            // The connection ended: the window goes with it. An unreachable Agent may be restarting; wait for it.
            if info["error"].is_null() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        };
        let prompts = self.send(&Request::Prompts, &[]);
        let waiting: Vec<Value> = prompts
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["who"].as_str().is_some_and(|w| rcli_agent::names(&who, w)))
            .cloned()
            .collect();

        egui::TopBottomPanel::top("head")
            .frame(egui::Frame::new().fill(look::INK).inner_margin(egui::Margin::symmetric(16, 12)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&who).font(look::bold(16.0)).color(look::PAPER_3));
                let since = connected_for(connection["connected_secs"].as_u64().unwrap_or_default());
                let doing = connection["doing"].as_str().unwrap_or_default();
                ui.label(egui::RichText::new(format!("connected {since}, {doing}")).color(look::PAPER_2));
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(look::PAPER).inner_margin(egui::Margin::same(16))).show(ctx, |ui| {
            if !self.note.is_empty() {
                look::small(ui, &self.note);
            }
            for p in waiting {
                let id = p["id"].as_u64().unwrap_or_default();
                let what = match p["kind"].as_str() {
                    Some("root") => "asks for root access",
                    _ => "wants to connect",
                };
                egui::Frame::new().fill(look::AMBER).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(what).font(look::bold(15.0)).color(look::INK));
                    ui.horizontal(|ui| {
                        let id_text = id.to_string();
                        if look::primary(ui, "Accept").clicked() {
                            self.act(Request::PromptAnswer { id, accept: true }, &["agent", "prompt", "answer", &id_text, "accept"]);
                        }
                        if ui.button("Reject").clicked() {
                            self.act(Request::PromptAnswer { id, accept: false }, &[]);
                        }
                    });
                });
            }

            look::section(ui, "Allowed");
            for (name, label) in [("mouse-keyboard", "Mouse and keyboard"), ("terminal", "Terminal"), ("power", "Power")] {
                let mut on = connection["capabilities"][name.replace('-', "_")] == true;
                if ui.checkbox(&mut on, label).changed() {
                    let pick = |n: &str| (n == name).then_some(on);
                    let request = Request::CapabilitiesSet {
                        who: who.clone(),
                        mouse_keyboard: pick("mouse-keyboard"),
                        terminal: pick("terminal"),
                        power: pick("power"),
                    };
                    // Switching one off is open to the person at the Device; back on is root's.
                    let root = if on { vec!["agent", "capability", name, "on", "--controller", &who] } else { vec![] };
                    self.act(request, &root);
                }
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if connection["root"] == true {
                    look::muted(ui, "has root for this session");
                } else if ui.button("Grant root").clicked() {
                    self.act(Request::RootGrant { who: who.clone() }, &["agent", "root", "grant", &who]);
                }
                if look::primary(ui, "Disconnect").clicked() {
                    self.act(Request::Disconnect { who: who.clone() }, &[]);
                }
            });
        });
    }
}

/// "42 s", "5 min", "2 h 5 min".
fn connected_for(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}
