//! The Devices area (decision 16): this OS user's Address Book. Every action runs the `rcli` command itself in
//! process (`rcli --json devices`, `connect`, `forget`, `wake`, `shot`), so the window and `rcli devices` read and
//! write one Address Book. Live View opens the Dashboard; terminal opens the OS terminal with `rcli shell <id>`.

use base64::Engine;
use eframe::egui;
use serde_json::Value;

use super::look;
use crate::config;

pub struct Devices {
    /// `rcli devices` as last asked: on first view, on Refresh and after a change (each row costs a Server request).
    book: Option<Result<Value, String>>,
    id: String,
    password: String,
    name: String,
    /// Comma-separated.
    tags: String,
    remember: bool,
    /// The last screenshot and whose it is.
    shot: Option<(String, egui::TextureHandle)>,
    /// The entry whose name and tags are being edited: Device ID, name, comma-separated tags.
    editing: Option<(String, String, String)>,
    /// The `rcli` command running off the UI thread, and its result once done.
    busy: Option<(Job, std::sync::mpsc::Receiver<Result<Value, String>>)>,
}

impl Default for Devices {
    fn default() -> Self {
        Self {
            book: None,
            id: String::new(),
            password: String::new(),
            name: String::new(),
            tags: String::new(),
            remember: true,
            shot: None,
            editing: None,
            busy: None,
        }
    }
}

/// What a running `rcli` command was for, to show its result.
enum Job {
    Book,
    /// The Device Password goes back in the field if adding fails.
    Add { id: String, password: String },
    Shot(String),
    Wake(String),
    Forget(String),
}

impl Job {
    fn doing(&self) -> String {
        match self {
            Job::Book => "reading the Address Book…".into(),
            Job::Add { id, .. } => format!("adding {id}…"),
            Job::Shot(id) => format!("taking a screenshot of {id}…"),
            Job::Wake(id) => format!("waking {id}…"),
            Job::Forget(id) => format!("forgetting {id}…"),
        }
    }
}

impl Devices {
    /// Runs `rcli --json <args>` in process on its own thread, so a slow Server never freezes the window.
    fn start(&mut self, job: Job, args: &[&str], rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (args, rt, ctx) = (args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), rt.handle().clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let argv = ["rcli", "--json"].into_iter().chain(args.iter().map(String::as_str));
            let done = rt.block_on(crate::run(argv, &mut out)).map_err(|e| format!("{e:#}"));
            let _ = tx.send(done.map(|()| serde_json::from_slice(&out).unwrap_or(Value::Null)));
            ctx.request_repaint();
        });
        self.busy = Some((job, rx));
    }

    /// Shows the result of the finished command.
    fn finish(&mut self, job: Job, done: Result<Value, String>, ctx: &egui::Context, note: &mut String) {
        match (job, done) {
            (Job::Book, done) => self.book = Some(done),
            (Job::Add { id, .. }, Ok(_)) => {
                *note = format!("added {id}");
                (self.id, self.name, self.tags) = Default::default();
                self.book = None;
            }
            (Job::Add { password, .. }, Err(e)) => {
                *note = e;
                self.password = password;
            }
            (Job::Shot(id), done) => match done.and_then(|obs| texture(ctx, &id, &obs)) {
                Ok(t) => self.shot = Some((id, t)),
                Err(e) => *note = e,
            },
            (Job::Wake(id), done) => {
                *note = match done {
                    Ok(w) => format!("waking {id} via {}", w["relay"]["name"].as_str().unwrap_or("-")),
                    Err(e) => e,
                }
            }
            (Job::Forget(id), done) => {
                *note = match done {
                    Ok(_) => format!("forgot {id}"),
                    Err(e) => e,
                };
                self.book = None;
            }
        }
    }

    /// Draws the area; the result of an action goes to `note`.
    pub fn show(&mut self, ui: &mut egui::Ui, rt: &tokio::runtime::Runtime, note: &mut String) {
        let ctx = ui.ctx().clone();
        if let Some((_, rx)) = &self.busy {
            if let Ok(done) = rx.try_recv() {
                let (job, _) = self.busy.take().expect("busy");
                self.finish(job, done, &ctx, note);
            }
        }
        if self.book.is_none() && self.busy.is_none() {
            self.start(Job::Book, &["devices"], rt, &ctx);
        }
        if let Some((job, _)) = &self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                look::muted(ui, &job.doing());
            });
        }
        // One command at a time: the buttons wait for the running one.
        ui.add_enabled_ui(self.busy.is_none(), |ui| self.controls(ui, rt, &ctx, note));
    }

    fn controls(&mut self, ui: &mut egui::Ui, rt: &tokio::runtime::Runtime, ctx: &egui::Context, note: &mut String) {
        look::section(ui, "Add a Device");
        egui::Grid::new("add-device").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            look::muted(ui, "Device ID");
            ui.text_edit_singleline(&mut self.id);
            ui.end_row();
            look::muted(ui, "Device Password");
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
            ui.end_row();
            look::muted(ui, "Name");
            ui.text_edit_singleline(&mut self.name);
            ui.end_row();
            look::muted(ui, "Tags");
            ui.add(egui::TextEdit::singleline(&mut self.tags).hint_text("comma-separated"));
            ui.end_row();
        });
        ui.checkbox(&mut self.remember, "Remember the Device Password in the OS keychain");
        if look::primary(ui, "Add").clicked() {
            let (id, password) = (self.id.trim().to_owned(), std::mem::take(&mut self.password));
            let (name, tags) = (self.name.trim().to_owned(), self.tags.clone());
            let mut args = vec!["connect", &id, "--password", &password, "--name", &name];
            for tag in tags.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                args.extend(["--tag", tag]);
            }
            if !self.remember {
                args.push("--no-remember");
            }
            let job = Job::Add { id: id.clone(), password: password.clone() };
            self.start(job, &args, rt, ctx);
        }

        look::section(ui, "Address Book");
        if ui.button("Refresh").clicked() {
            self.book = None;
        }
        let rows = match &self.book {
            Some(Ok(Value::Array(rows))) => rows.clone(),
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).color(look::ERROR));
                vec![]
            }
            _ => vec![],
        };
        if rows.is_empty() {
            look::muted(ui, "No Devices yet: add one by its Device ID and Device Password.");
        }
        for d in &rows {
            let id = d["device_id"].as_str().unwrap_or_default();
            let tags: Vec<&str> = d["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                look::signal(ui, d["status"] == "Online", look::INK);
                ui.label(egui::RichText::new(id).font(look::bold(15.0)));
                ui.label(d["name"].as_str().unwrap_or_default());
                if !tags.is_empty() {
                    look::small(ui, &tags.join(", "));
                }
                look::small(ui, d["error"].as_str().or(d["status"].as_str()).unwrap_or("not asked: no remembered password"));
            });
            if let Some((_, name, tags)) = self.editing.as_mut().filter(|(editing, ..)| editing == id) {
                ui.horizontal(|ui| {
                    look::muted(ui, "Name");
                    ui.text_edit_singleline(name);
                    look::muted(ui, "Tags");
                    ui.add(egui::TextEdit::singleline(tags).hint_text("comma-separated"));
                });
            }
            ui.horizontal(|ui| {
                if ui.button("Screenshot").clicked() {
                    self.start(Job::Shot(id.to_owned()), &["shot", id], rt, ctx);
                }
                if ui.button("Wake").clicked() {
                    self.start(Job::Wake(id.to_owned()), &["wake", id], rt, ctx);
                }
                match self.editing.take() {
                    Some((editing, name, tags)) if editing == id => {
                        if look::primary(ui, "Save").clicked() {
                            // The entry `rcli connect --name --tag` writes, with no new password check.
                            let tags = tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect();
                            let entry = config::Entry { device_id: editing, name: name.trim().to_owned(), tags, wake: None };
                            *note = match config::add_entry(&config::dir(), entry) {
                                Ok(()) => format!("saved {id}"),
                                Err(e) => format!("cannot save {id}: {e:#}"),
                            };
                            self.book = None;
                        } else if !ui.button("Cancel").clicked() {
                            self.editing = Some((editing, name, tags));
                        }
                    }
                    other => {
                        self.editing = other;
                        if ui.button("Edit").clicked() {
                            let name = d["name"].as_str().unwrap_or_default().to_owned();
                            self.editing = Some((id.to_owned(), name, tags.join(", ")));
                        }
                    }
                }
                if ui.button("Live View").clicked() {
                    if let Err(e) = live_view(id) {
                        *note = format!("{e:#}");
                    }
                }
                if ui.button("Terminal").clicked() {
                    if let Err(e) = terminal(id) {
                        *note = format!("cannot open a terminal: {e:#}");
                    }
                }
                if ui.button("Forget").clicked() {
                    self.start(Job::Forget(id.to_owned()), &["forget", id], rt, ctx);
                }
            });
            look::rule(ui, look::RULE, 1.0);
        }

        if let Some((id, shot)) = &self.shot {
            look::section(ui, &format!("Screenshot of {id}"));
            ui.add(egui::Image::from_texture(shot).max_width(ui.available_width()));
        }
    }
}

/// The first image of an Observation, as a texture.
fn texture(ctx: &egui::Context, id: &str, obs: &Value) -> Result<egui::TextureHandle, String> {
    let data = obs["images"][0]["data"].as_str().ok_or("the Device sent no image")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Ok(ctx.load_texture(format!("shot-{id}"), pixels, Default::default()))
}

/// Opens the Device's Live View in the Dashboard (the API Address is the Dashboard's URL).
fn live_view(id: &str) -> anyhow::Result<()> {
    let dir = config::dir();
    let machine = config::installed(&config::controller_path(&dir, None))?;
    let url = format!("{}/devices/{id}/screen", config::api_address(&machine)?.trim_end_matches('/'));
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn()?;
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(&url).spawn()?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn()?;
    Ok(())
}

/// Opens the OS terminal running `rcli shell <id>`.
fn terminal(id: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    {
        let command = format!("{} shell {}", super::shell_quote(&exe.to_string_lossy()), super::shell_quote(id));
        let script = format!("tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell", command.replace('\\', "\\\\").replace('"', "\\\""));
        std::process::Command::new("osascript").arg("-e").arg(script).spawn()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        std::process::Command::new(&exe).args(["shell", id]).creation_flags(CREATE_NEW_CONSOLE).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        // The Debian alternative first, then the common desktops' own terminals.
        let terminals = [("x-terminal-emulator", "-e"), ("gnome-terminal", "--"), ("konsole", "-e"), ("xterm", "-e")];
        let opened = terminals.iter().any(|(term, flag)| std::process::Command::new(term).arg(flag).arg(&exe).args(["shell", id]).spawn().is_ok());
        anyhow::ensure!(opened, "no terminal found (tried x-terminal-emulator, gnome-terminal, konsole, xterm)");
    }
    Ok(())
}
