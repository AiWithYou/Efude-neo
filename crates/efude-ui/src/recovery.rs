// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Crash recovery for every dirty tab, including canvases that were never saved.
//! Recovery snapshots are separate from the user's project files and backups.
use super::*;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

// Accommodate extended Windows paths, including JSON/UTF-8 expansion,
// while keeping recovery sidecar reads bounded.
const MAX_METADATA_BYTES: u64 = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SnapshotMeta {
    pub version: u32,
    pub document_id: u64,
    pub state_token: u64,
    pub title: String,
    pub original_path: Option<PathBuf>,
    pub tab_index: usize,
    pub saved_at: u64,
    #[serde(default)]
    pub saved_at_nanos: u128,
    pub width: u32,
    pub height: u32,
}

pub(crate) struct Candidate {
    pub snapshot: PathBuf,
    pub metadata: PathBuf,
    pub info: SnapshotMeta,
    pub selected: bool,
    pub loading: bool,
    pub thumbnail: Option<egui::TextureHandle>,
    thumbnail_attempted: bool,
}

pub(crate) struct Store {
    root: PathBuf,
    current: Option<PathBuf>,
    _lease: Option<File>,
    pub candidates: Vec<Candidate>,
    pub show_dialog: bool,
    pub confirm_discard: bool,
    pub queued_tokens: HashMap<u64, u64>,
    restored: Vec<(PathBuf, PathBuf)>,
}

#[cfg(not(test))]
fn root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Efude-neo")
        .join("recovery")
}

fn exclusive_open(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(path)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn is_run_folder(root: &Path, path: &Path) -> bool {
    let direct_child = path.parent() == Some(root)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("run-"));
    direct_child
        && fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_symlink())
        && root
            .canonicalize()
            .ok()
            .zip(path.canonicalize().ok())
            .is_some_and(|(root, path)| path.parent() == Some(root.as_path()))
}

impl Store {
    pub fn new() -> Self {
        #[cfg(test)]
        {
            static TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let id = TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Self::new_in(std::env::temp_dir().join(format!(
                "efude-neo-recovery-test-{}-{id}",
                std::process::id()
            )))
        }
        #[cfg(not(test))]
        {
            Self::new_in(root())
        }
    }

    pub fn new_in(root: PathBuf) -> Self {
        let mut store = Self {
            root,
            current: None,
            _lease: None,
            candidates: Vec::new(),
            show_dialog: false,
            confirm_discard: false,
            queued_tokens: HashMap::new(),
            restored: Vec::new(),
        };
        if fs::create_dir_all(&store.root).is_err() {
            return store;
        }
        store.scan_old_sessions();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        for attempt in 0..100u32 {
            let candidate = store
                .root
                .join(format!("run-{stamp}-{}-{attempt}", std::process::id()));
            if fs::create_dir(&candidate).is_ok()
                && let Ok(lease) = exclusive_open(&candidate.join("lease"))
            {
                store._lease = Some(lease);
                store.current = Some(candidate);
                break;
            }
        }
        store.show_dialog = !store.candidates.is_empty();
        store
    }

    fn scan_old_sessions(&mut self) {
        let Ok(runs) = fs::read_dir(&self.root) else {
            return;
        };
        for run in runs.flatten() {
            let folder = run.path();
            if !is_run_folder(&self.root, &folder) || !folder.is_dir() {
                continue;
            }
            if folder.join("closed").exists() {
                let _ = fs::remove_dir_all(&folder);
                continue;
            }
            // A live second instance holds this file without sharing on Windows.
            if exclusive_open(&folder.join("lease")).is_err() {
                continue;
            }
            let Ok(items) = fs::read_dir(&folder) else {
                continue;
            };
            for item in items.flatten() {
                let metadata = item.path();
                if metadata
                    .extension()
                    .is_none_or(|extension| extension != "json")
                    || !metadata
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("tab_"))
                    || metadata
                        .metadata()
                        .is_ok_and(|data| data.len() > MAX_METADATA_BYTES)
                {
                    continue;
                }
                let snapshot = metadata.with_extension("efude");
                if !snapshot.is_file() {
                    continue;
                }
                let Ok(bytes) = fs::read(&metadata) else {
                    continue;
                };
                let Ok(info) = serde_json::from_slice::<SnapshotMeta>(&bytes) else {
                    continue;
                };
                if !valid_snapshot_metadata(&metadata, &info) {
                    continue;
                }
                self.candidates.push(Candidate {
                    snapshot,
                    metadata,
                    info,
                    selected: true,
                    loading: false,
                    thumbnail: None,
                    thumbnail_attempted: false,
                });
            }
        }
        self.candidates.sort_by(|a, b| {
            (
                a.snapshot.parent().map(Path::to_path_buf),
                a.info.tab_index,
                std::cmp::Reverse(a.info.saved_at_nanos),
            )
                .cmp(&(
                    b.snapshot.parent().map(Path::to_path_buf),
                    b.info.tab_index,
                    std::cmp::Reverse(b.info.saved_at_nanos),
                ))
        });
        let mut newest = std::collections::HashSet::new();
        for candidate in &mut self.candidates {
            candidate.selected = newest.insert((
                candidate.snapshot.parent().map(Path::to_path_buf),
                candidate.info.document_id,
            ));
        }
    }

    pub fn snapshot_paths(&self, document_id: u64, state_token: u64) -> Option<(PathBuf, PathBuf)> {
        let base = self
            .current
            .as_ref()?
            .join(format!("tab_{document_id}_{state_token}"));
        Some((base.with_extension("efude"), base.with_extension("json")))
    }

    pub fn current_dir(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    pub fn remember_restored(&mut self, snapshot: PathBuf, metadata: PathBuf) {
        let document_id = self
            .candidates
            .iter()
            .find(|candidate| candidate.snapshot == snapshot)
            .map(|candidate| candidate.info.document_id);
        let folder = snapshot.parent().map(Path::to_path_buf);
        self.restored.push((snapshot, metadata));
        for candidate in &self.candidates {
            if Some(candidate.info.document_id) == document_id
                && candidate.snapshot.parent() == folder.as_deref()
            {
                self.restored
                    .push((candidate.snapshot.clone(), candidate.metadata.clone()));
            }
        }
        self.candidates.retain(|candidate| {
            Some(candidate.info.document_id) != document_id
                || candidate.snapshot.parent() != folder.as_deref()
        });
    }

    pub fn discard_selected(&mut self) -> usize {
        let mut removed = 0;
        self.candidates.retain(|candidate| {
            if candidate.selected && !candidate.loading {
                let _ = fs::remove_file(&candidate.snapshot);
                let _ = fs::remove_file(&candidate.metadata);
                removed += 1;
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn finish_cleanly(&mut self) {
        if let Some(current) = self.current.take()
            && is_run_folder(&self.root, &current)
        {
            let _ = File::create(current.join("closed"));
            self._lease.take();
            let _ = fs::remove_dir_all(current);
        }
        for (snapshot, metadata) in self.restored.drain(..) {
            if snapshot
                .parent()
                .is_some_and(|parent| is_run_folder(&self.root, parent))
                && metadata.parent() == snapshot.parent()
            {
                let _ = fs::remove_file(snapshot);
                let _ = fs::remove_file(metadata);
            }
        }
    }

    pub fn ensure_thumbnails(&mut self, ctx: &egui::Context) {
        for candidate in &mut self.candidates {
            if candidate.thumbnail_attempted {
                continue;
            }
            candidate.thumbnail_attempted = true;
            if let Ok((width, height, rgba)) = efude_io::load_thumbnail(&candidate.snapshot)
                && width > 0
                && height > 0
                && width <= 256
                && height <= 256
            {
                candidate.thumbnail = Some(ctx.load_texture(
                    format!("recovery-{}", candidate.snapshot.display()),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
    }
}

/// Every state has a distinct pair of files. A failed new write leaves the
/// previous complete generation available for recovery.
pub(crate) fn write_snapshot(
    snapshot: &Path,
    metadata: &Path,
    doc: &Document,
    info: &SnapshotMeta,
) -> Result<(), String> {
    let parent = metadata.parent().ok_or("invalid recovery metadata path")?;
    if !valid_snapshot_metadata(metadata, info)
        || snapshot != metadata.with_extension("efude").as_path()
        || info.width != doc.width
        || info.height != doc.height
    {
        return Err("invalid recovery snapshot identity".into());
    }
    let previous = read_generation_metadata(parent, info.document_id)?
        .into_iter()
        .map(|(_, info)| info.saved_at_nanos)
        .max();
    let mut stored_info = info.clone();
    if let Some(previous) = previous {
        stored_info.saved_at_nanos = stored_info.saved_at_nanos.max(
            previous
                .checked_add(1)
                .ok_or("Recovery generation order overflow")?,
        );
    }
    // `saved_at` remains the real wall-clock time shown by the UI. The
    // nanosecond field also orders generations, including retries and Undo.
    let bytes = serde_json::to_vec(&stored_info).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("Recovery metadata is too large".into());
    }
    efude_io::save(snapshot, doc).map_err(|error| error.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temp.write_all(&bytes).map_err(|error| error.to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temp.persist(metadata).map_err(|error| error.to_string())?;
    prune_old_generations(parent, info.document_id);
    Ok(())
}

fn generation_metadata(folder: &Path, document_id: u64) -> Vec<(PathBuf, SnapshotMeta)> {
    read_generation_metadata(folder, document_id).unwrap_or_default()
}

fn valid_snapshot_metadata(metadata: &Path, info: &SnapshotMeta) -> bool {
    info.version == 1
        && efude_canvas::valid_document_dimensions(info.width, info.height)
        && metadata
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == format!("tab_{}_{}.json", info.document_id, info.state_token)
            })
}

fn read_generation_metadata(
    folder: &Path,
    document_id: u64,
) -> Result<Vec<(PathBuf, SnapshotMeta)>, String> {
    let items = fs::read_dir(folder).map_err(|error| error.to_string())?;
    let prefix = format!("tab_{document_id}_");
    let mut generations = Vec::new();
    for item in items {
        let item = item.map_err(|error| error.to_string())?;
        let path = item.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        if !item
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            || path.metadata().map_err(|error| error.to_string())?.len() > MAX_METADATA_BYTES
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let Ok(info) = serde_json::from_slice::<SnapshotMeta>(&bytes) else {
            continue;
        };
        if info.document_id != document_id || !valid_snapshot_metadata(&path, &info) {
            continue;
        }
        match fs::symlink_metadata(path.with_extension("efude")) {
            Ok(data) if data.is_file() => generations.push((path, info)),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(generations)
}

fn prune_old_generations(folder: &Path, document_id: u64) {
    let mut generations = generation_metadata(folder, document_id);
    generations.sort_by_key(|(_, info)| (info.saved_at_nanos, info.state_token));
    let drop_count = generations.len().saturating_sub(2);
    for (metadata, _) in generations.into_iter().take(drop_count) {
        let _ = fs::remove_file(metadata.with_extension("efude"));
        let _ = fs::remove_file(metadata);
    }
}

pub(crate) fn clear_snapshot_family(folder: &Path, document_id: u64, saved_token: Option<u64>) {
    for (metadata, info) in generation_metadata(folder, document_id) {
        if saved_token.is_none_or(|token| token == info.state_token) {
            let _ = fs::remove_file(metadata.with_extension("efude"));
            let _ = fs::remove_file(metadata);
        }
    }
}

pub(crate) fn make_metadata(
    document_id: u64,
    state_token: u64,
    title: String,
    original_path: Option<PathBuf>,
    tab_index: usize,
    doc: &Document,
) -> SnapshotMeta {
    SnapshotMeta {
        version: 1,
        document_id,
        state_token,
        title,
        original_path,
        tab_index,
        saved_at: now_seconds(),
        saved_at_nanos: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos()),
        width: doc.width,
        height: doc.height,
    }
}

impl EfudeApp {
    pub(crate) fn queue_recovery_snapshots(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<usize, String> {
        self.commit_pending_guide_edit();
        let mut snapshots = Vec::new();
        for index in 0..self.tabs.slots.len() {
            let (doc, history, path) = match &self.tabs.slots[index].parked {
                Some(tab) => (&tab.doc, &tab.history, tab.doc_path.as_ref()),
                None => (&self.doc, &self.history, self.doc_path.as_ref()),
            };
            if !history.is_dirty()
                || self.recovery.queued_tokens.get(&history.document_id())
                    == Some(&history.state_token())
            {
                continue;
            }
            let Some((snapshot, metadata)) = self
                .recovery
                .snapshot_paths(history.document_id(), history.state_token())
            else {
                continue;
            };
            let info = make_metadata(
                history.document_id(),
                history.state_token(),
                self.tab_title(index),
                path.cloned(),
                index,
                doc,
            );
            snapshots.push((snapshot, metadata, info, doc.clone()));
        }
        let mut count = 0;
        for (snapshot, metadata, info, document) in snapshots {
            let document_id = info.document_id;
            let token = info.state_token;
            self.io_task_sender
                .send(IoTask::SaveRecovery {
                    snapshot,
                    metadata,
                    info,
                    document,
                    repaint: ctx.clone(),
                })
                .map_err(|_| "作業復旧データを保存できません".to_string())?;
            self.recovery.queued_tokens.insert(document_id, token);
            count += 1;
        }
        Ok(count)
    }

    pub(crate) fn recovery_dialog(&mut self, ctx: &egui::Context) {
        if !self.recovery.show_dialog {
            return;
        }
        self.recovery.ensure_thumbnails(ctx);
        let english = self.language_english;
        let mut restore = false;
        let mut later = false;
        let mut discard = false;
        let mut open = true;
        egui::Window::new(self.text("作業の復旧", "Recover work"))
            .open(&mut open)
            .default_width(470.0)
            .resizable(true)
            .show(ctx, |ui| {
                if self.recovery.candidates.is_empty() {
                    ui.label(self.text(
                        "復旧できる作業はありません。",
                        "No recoverable work was found.",
                    ));
                } else {
                    ui.label(self.text(
                        "復元した作品は新しいタブで開きます。元のファイルは変更しません。",
                        "Recovered work opens in new tabs. Original files are not changed.",
                    ));
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(340.0)
                        .show(ui, |ui| {
                            for candidate in &mut self.recovery.candidates {
                                ui.horizontal(|ui| {
                                    ui.add_enabled(
                                        !candidate.loading,
                                        egui::Checkbox::new(&mut candidate.selected, ""),
                                    );
                                    if let Some(thumbnail) = &candidate.thumbnail {
                                        let size = thumbnail.size_vec2();
                                        let scale = (76.0 / size.x.max(size.y)).min(1.0);
                                        ui.image((thumbnail.id(), size * scale));
                                    } else {
                                        ui.allocate_space(Vec2::new(76.0, 60.0));
                                    }
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(&candidate.info.title).strong(),
                                        );
                                        if let Some(path) = &candidate.info.original_path {
                                            ui.small(path.display().to_string());
                                        } else {
                                            ui.small(if english {
                                                "Never saved"
                                            } else {
                                                "一度も保存されていません"
                                            });
                                        }
                                        let age_minutes = now_seconds()
                                            .saturating_sub(candidate.info.saved_at)
                                            / 60;
                                        let age = if age_minutes < 60 {
                                            format!("{age_minutes}m")
                                        } else if age_minutes < 1440 {
                                            format!("{}h", age_minutes / 60)
                                        } else {
                                            format!("{}d", age_minutes / 1440)
                                        };
                                        ui.small(format!(
                                            "{} × {} · {} · tab {}",
                                            candidate.info.width,
                                            candidate.info.height,
                                            age,
                                            candidate.info.tab_index as u128 + 1
                                        ));
                                    });
                                });
                                ui.separator();
                            }
                        });
                    let selected = self
                        .recovery
                        .candidates
                        .iter()
                        .any(|item| item.selected && !item.loading);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                selected,
                                egui::Button::new(if english {
                                    "Restore selected"
                                } else {
                                    "選択した作品を復元"
                                }),
                            )
                            .clicked()
                        {
                            restore = true;
                        }
                        if ui.button(if english { "Later" } else { "後で" }).clicked() {
                            later = true;
                        }
                        if ui
                            .add_enabled(
                                selected,
                                egui::Button::new(if english {
                                    "Discard selected…"
                                } else {
                                    "選択を破棄…"
                                }),
                            )
                            .clicked()
                        {
                            discard = true;
                        }
                    });
                }
            });
        if !open || later {
            self.recovery.show_dialog = false;
        }
        if discard {
            self.recovery.confirm_discard = true;
        }
        if restore {
            for candidate in self
                .recovery
                .candidates
                .iter_mut()
                .filter(|item| item.selected && !item.loading)
            {
                let task = IoTask::LoadRecovery {
                    snapshot: candidate.snapshot.clone(),
                    metadata: candidate.metadata.clone(),
                    info: candidate.info.clone(),
                    repaint: ctx.clone(),
                };
                if self.io_task_sender.send(task).is_ok() {
                    candidate.loading = true;
                }
            }
            self.recovery.show_dialog = false;
            self.status = self.text("作業を復元中…", "Recovering work…").into();
        }
        if self.recovery.confirm_discard {
            let mut confirm_open = true;
            let mut confirmed = false;
            egui::Window::new(self.text("復旧データを破棄", "Discard recovery data"))
                .open(&mut confirm_open)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(self.text(
                        "選択した復旧データを削除します。この操作は取り消せません。",
                        "Selected recovery data will be deleted. This cannot be undone.",
                    ));
                    if ui.button(self.text("破棄する", "Discard")).clicked() {
                        confirmed = true;
                    }
                });
            if confirmed {
                let count = self.recovery.discard_selected();
                self.status = if english {
                    format!("Discarded {count} recovery snapshots")
                } else {
                    format!("{count}件の復旧データを破棄しました")
                };
                self.recovery.confirm_discard = false;
                self.recovery.show_dialog = !self.recovery.candidates.is_empty();
            } else if !confirm_open {
                self.recovery.confirm_discard = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_pixel(app: &mut EfudeApp, color: [u8; 4]) {
        app.history.begin();
        app.history.record_pixel(&app.doc.layers[0], 0);
        app.doc.layers[0].pixels.set_pixel(0, 0, color);
        app.history.commit();
    }

    fn finish_io(app: &mut EfudeApp, ctx: &egui::Context) {
        for _ in 0..200 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.update_ui(ctx));
            if app.io_task_sender.busy.get() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("I/O did not finish: {}", app.status);
    }

    #[test]
    fn recovery_snapshots_resume_after_saving_then_undoing() {
        for save_in_parked_tab in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut app = EfudeApp::default();
            app.recovery.finish_cleanly();
            app.recovery = Store::new_in(root.path().join("recovery"));
            app.doc = Document::new(8, 8);
            let ctx = egui::Context::default();
            paint_pixel(&mut app, [10, 20, 30, 255]);
            let document_id = app.history.document_id();
            let first_token = app.history.state_token();
            let (snapshot, _) = app
                .recovery
                .snapshot_paths(document_id, first_token)
                .unwrap();
            assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 1);
            finish_io(&mut app, &ctx);
            assert!(snapshot.exists());

            paint_pixel(&mut app, [40, 50, 60, 255]);
            app.queue_document_save(root.path().join("saved.efude"), false, &ctx)
                .unwrap();
            if save_in_parked_tab {
                app.open_document_tab();
            }
            finish_io(&mut app, &ctx);
            if save_in_parked_tab {
                app.switch_tab(0);
            }
            assert!(!app.history.is_dirty());
            assert!(!snapshot.exists());

            app.history.undo_document(&mut app.doc);
            assert_eq!(app.history.state_token(), first_token);
            assert!(app.history.is_dirty());
            assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 1);
            finish_io(&mut app, &ctx);
            assert_eq!(
                efude_io::load(&snapshot).unwrap().layers[0]
                    .pixels
                    .pixel(0, 0),
                [10, 20, 30, 255]
            );
        }
    }

    #[test]
    fn recovery_keeps_a_newer_snapshot_queued_while_saving() {
        let root = tempfile::tempdir().unwrap();
        let mut app = EfudeApp::default();
        app.recovery.finish_cleanly();
        app.recovery = Store::new_in(root.path().join("recovery"));
        app.doc = Document::new(8, 8);
        let ctx = egui::Context::default();
        paint_pixel(&mut app, [10, 20, 30, 255]);
        let document_id = app.history.document_id();
        let saved_token = app.history.state_token();
        let (saved_snapshot, _) = app
            .recovery
            .snapshot_paths(document_id, saved_token)
            .unwrap();
        assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 1);
        app.queue_document_save(root.path().join("saved.efude"), false, &ctx)
            .unwrap();

        // Queue another generation before processing the save completion.
        paint_pixel(&mut app, [40, 50, 60, 255]);
        let newer_token = app.history.state_token();
        let (newer_snapshot, _) = app
            .recovery
            .snapshot_paths(document_id, newer_token)
            .unwrap();
        assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 1);
        finish_io(&mut app, &ctx);
        assert!(app.history.is_dirty());
        assert!(!saved_snapshot.exists());
        assert_eq!(
            efude_io::load(&newer_snapshot).unwrap().layers[0]
                .pixels
                .pixel(0, 0),
            [40, 50, 60, 255]
        );
        assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 0);
    }

    #[test]
    fn two_unsaved_tabs_survive_an_unclean_session_and_keep_their_thumbnails() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Store::new_in(root.path().to_path_buf());
        for (id, color) in [(101, [255, 0, 0, 255]), (102, [0, 0, 255, 255])] {
            let mut doc = Document::new(16, 16);
            doc.layers[0].pixels.set_pixel(3, 3, color);
            let (snapshot, metadata_path) = first.snapshot_paths(id, id).unwrap();
            let info = make_metadata(id, id, format!("無題{id}"), None, (id - 101) as usize, &doc);
            write_snapshot(&snapshot, &metadata_path, &doc, &info).unwrap();
        }
        first._lease.take(); // Simulate the OS releasing the lock after a crash.
        let mut restarted = Store::new_in(root.path().to_path_buf());
        assert_eq!(restarted.candidates.len(), 2);
        assert!(restarted.show_dialog);
        for candidate in &restarted.candidates {
            assert!(candidate.info.original_path.is_none());
            assert!(efude_io::load_thumbnail(&candidate.snapshot).is_ok());
            let doc = efude_io::load(&candidate.snapshot).unwrap();
            let expected = if candidate.info.document_id == 101 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            assert_eq!(doc.layers[0].pixels.pixel(3, 3), expected);
        }
        restarted.finish_cleanly();
    }

    #[test]
    fn long_valid_document_titles_remain_available_for_recovery() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        // A valid file name can exceed 240 characters, and recovered-tab titles
        // additionally include a suffix. Neither should hide a complete snapshot.
        let title = format!("{}.efude (recovered)", "a".repeat(245));
        let original = root.path().join(format!("{}.efude", "a".repeat(245)));
        let info = make_metadata(1, 2, title.clone(), Some(original), 0, &doc);
        let (snapshot, metadata) = first.snapshot_paths(1, 2).unwrap();
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        first._lease.take();

        let mut restarted = Store::new_in(root.path().to_path_buf());
        assert_eq!(restarted.candidates.len(), 1);
        assert_eq!(restarted.candidates[0].info.title, title);
        assert!(restarted.candidates[0].selected);
        restarted.finish_cleanly();
    }

    #[test]
    fn recovery_dialog_can_display_the_largest_imported_tab_index() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set_pixel(0, 0, [12, 34, 56, 255]);
        let info = make_metadata(1, 2, "Recovered".into(), None, usize::MAX, &doc);
        let (snapshot, metadata) = first.snapshot_paths(1, 2).unwrap();
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        first._lease.take();

        let mut app = EfudeApp::default();
        app.recovery.finish_cleanly();
        app.recovery = Store::new_in(root.path().to_path_buf());
        assert_eq!(app.recovery.candidates.len(), 1);
        assert_eq!(app.recovery.candidates[0].info.tab_index, usize::MAX);
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| app.recovery_dialog(ctx));
        assert_eq!(
            efude_io::load(&snapshot).unwrap().layers[0]
                .pixels
                .pixel(0, 0),
            [12, 34, 56, 255]
        );
        app.recovery.finish_cleanly();
    }

    #[test]
    fn long_original_paths_remain_available_for_recovery_and_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        // Windows extended paths permit up to 32,767 UTF-16 units. Each
        // component here stays below the usual 255-unit component limit.
        let mut original = PathBuf::from(r"\\?\C:\");
        for _ in 0..100 {
            original.push("a".repeat(180));
        }
        original.push("drawing.efude");
        let info = make_metadata(
            1,
            2,
            "drawing.efude".into(),
            Some(original.clone()),
            0,
            &doc,
        );
        let (snapshot, metadata) = first.snapshot_paths(1, 2).unwrap();
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        assert!(fs::metadata(&metadata).unwrap().len() > 16 * 1024);
        assert_eq!(
            generation_metadata(first.current_dir().unwrap(), 1).len(),
            1
        );
        first._lease.take();

        let mut restarted = Store::new_in(root.path().to_path_buf());
        assert_eq!(restarted.candidates.len(), 1);
        assert_eq!(restarted.candidates[0].info.original_path, Some(original));
        clear_snapshot_family(snapshot.parent().unwrap(), 1, None);
        assert!(!snapshot.exists());
        assert!(!metadata.exists());
        restarted.finish_cleanly();
    }

    #[test]
    fn saved_state_clears_only_the_matching_recovery_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        let (snapshot, metadata_path) = store.snapshot_paths(1, 5).unwrap();
        write_snapshot(
            &snapshot,
            &metadata_path,
            &doc,
            &make_metadata(1, 5, "Test".into(), None, 0, &doc),
        )
        .unwrap();
        clear_snapshot_family(store.current_dir().unwrap(), 1, Some(4));
        assert!(snapshot.exists());
        let (newer, newer_meta) = store.snapshot_paths(1, 6).unwrap();
        write_snapshot(
            &newer,
            &newer_meta,
            &doc,
            &make_metadata(1, 6, "Test".into(), None, 0, &doc),
        )
        .unwrap();
        clear_snapshot_family(store.current_dir().unwrap(), 1, Some(5));
        assert!(!snapshot.exists());
        assert!(newer.exists());
        store.finish_cleanly();
    }

    #[test]
    fn a_failed_new_generation_keeps_the_previous_complete_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set_pixel(0, 0, [1, 2, 3, 255]);
        let (old, old_meta) = store.snapshot_paths(1, 10).unwrap();
        write_snapshot(
            &old,
            &old_meta,
            &doc,
            &make_metadata(1, 10, "A".into(), None, 0, &doc),
        )
        .unwrap();
        doc.layers[0].pixels.set_pixel(0, 0, [4, 5, 6, 255]);
        let (new, _) = store.snapshot_paths(1, 11).unwrap();
        let bad_meta = store
            .current_dir()
            .unwrap()
            .join("missing")
            .join("tab_1_11.json");
        assert!(
            write_snapshot(
                &new,
                &bad_meta,
                &doc,
                &make_metadata(1, 11, "A".into(), None, 0, &doc)
            )
            .is_err()
        );
        assert_eq!(
            efude_io::load(&old).unwrap().layers[0].pixels.pixel(0, 0),
            [1, 2, 3, 255]
        );
        assert!(old_meta.exists());
        store.finish_cleanly();
    }

    #[test]
    fn recovery_keeps_latest_saves_after_clock_moves_backwards() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        let future = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
            + std::time::Duration::from_secs(3600))
        .as_nanos();
        for (index, token) in [10, 20, 5].into_iter().enumerate() {
            doc.layers[0]
                .pixels
                .set_pixel(0, 0, [index as u8 + 1, 0, 0, 255]);
            let (snapshot, metadata) = store.snapshot_paths(1, token).unwrap();
            let mut info = make_metadata(1, token, "A".into(), None, 0, &doc);
            if index < 2 {
                info.saved_at_nanos = future + index as u128;
            } else {
                info.saved_at = 42;
            }
            write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        }
        let (latest, latest_meta) = store.snapshot_paths(1, 5).unwrap();
        assert!(latest.exists(), "the newest saved recovery was pruned");
        assert!(store.snapshot_paths(1, 20).unwrap().0.exists());
        assert!(!store.snapshot_paths(1, 10).unwrap().0.exists());
        let info: SnapshotMeta = serde_json::from_slice(&fs::read(&latest_meta).unwrap()).unwrap();
        assert_eq!(
            info.saved_at, 42,
            "the UI must retain the real wall-clock time"
        );
        assert!(info.saved_at_nanos > future + 1);

        store.current = None; // Leave the files as an interrupted process would.
        drop(store);
        let mut recovered = Store::new_in(root.path().to_path_buf());
        let selected = recovered
            .candidates
            .iter()
            .find(|candidate| candidate.selected)
            .unwrap();
        assert_eq!(selected.snapshot, latest);
        assert_eq!(
            efude_io::load(&selected.snapshot).unwrap().layers[0]
                .pixels
                .pixel(0, 0),
            [3, 0, 0, 255]
        );
        recovered.finish_cleanly();
    }

    #[test]
    fn recovery_retry_updates_same_state_without_changing_display_time() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        let (snapshot, metadata) = store.snapshot_paths(1, 5).unwrap();
        let mut info = make_metadata(1, 5, "A".into(), None, 0, &doc);
        info.saved_at_nanos = 100;
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        doc.layers[0].pixels.set_pixel(0, 0, [4, 5, 6, 128]);
        info.saved_at_nanos = 1;
        info.saved_at = 42;
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        let stored: SnapshotMeta = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        assert_eq!(stored.saved_at_nanos, 101);
        assert_eq!(stored.saved_at, 42);
        assert_eq!(stored.state_token, 5);
        assert_eq!(
            info.saved_at_nanos, 1,
            "the requested metadata is not mutated"
        );
        assert_eq!(
            efude_io::load(&snapshot).unwrap().layers[0]
                .pixels
                .pixel(0, 0),
            [4, 5, 6, 128]
        );
        assert_eq!(
            generation_metadata(store.current_dir().unwrap(), 1).len(),
            1
        );
        store.finish_cleanly();
    }

    #[test]
    fn recovery_order_overflow_preserves_existing_generations_and_original() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        let original = root.path().join("original.efude");
        efude_io::save(&original, &doc).unwrap();
        let mut preserved = vec![(original.clone(), fs::read(&original).unwrap())];
        for (token, stamp) in [(10, u128::MAX - 1), (20, u128::MAX)] {
            let (snapshot, metadata) = store.snapshot_paths(1, token).unwrap();
            let mut info = make_metadata(1, token, "A".into(), Some(original.clone()), 0, &doc);
            info.saved_at_nanos = stamp;
            write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
            preserved.push((snapshot.clone(), fs::read(&snapshot).unwrap()));
            preserved.push((metadata.clone(), fs::read(&metadata).unwrap()));
        }
        doc.layers[0].pixels.set_pixel(0, 0, [4, 5, 6, 255]);
        // Check both a new generation and a retry of the existing maximum.
        for token in [5, 20] {
            let (snapshot, metadata) = store.snapshot_paths(1, token).unwrap();
            let mut info = make_metadata(1, token, "A".into(), Some(original.clone()), 0, &doc);
            info.saved_at_nanos = 1;
            assert_eq!(
                write_snapshot(&snapshot, &metadata, &doc, &info).unwrap_err(),
                "Recovery generation order overflow"
            );
            for (path, bytes) in &preserved {
                assert_eq!(
                    &fs::read(path).unwrap(),
                    bytes,
                    "changed {}",
                    path.display()
                );
            }
        }
        let (new_snapshot, new_metadata) = store.snapshot_paths(1, 5).unwrap();
        assert!(!new_snapshot.exists());
        assert!(!new_metadata.exists());
        assert_eq!(
            generation_metadata(store.current_dir().unwrap(), 1).len(),
            2
        );
        store.finish_cleanly();
    }

    #[test]
    fn recovery_order_ignores_foreign_invalid_and_incomplete_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        let folder = store.current_dir().unwrap();
        let mut untouched = Vec::new();
        for token in 100..=106 {
            let mut info = make_metadata(1, token, "A".into(), None, 0, &doc);
            info.saved_at_nanos = u128::MAX;
            let filename = if token == 104 {
                "tab_10_104".to_string()
            } else {
                format!("tab_1_{token}")
            };
            match token {
                100 => info.version = 2,
                101 => info.document_id = 2,
                102 => info.state_token = 999,
                103 => info.width = 0,
                104 => info.document_id = 10,
                _ => {}
            }
            let metadata = folder.join(format!("{filename}.json"));
            let bytes = if token == 105 {
                b"{broken".to_vec()
            } else {
                serde_json::to_vec(&info).unwrap()
            };
            fs::write(&metadata, &bytes).unwrap();
            untouched.push((metadata, bytes));
            if token != 106 {
                let snapshot = folder.join(format!("{filename}.efude"));
                efude_io::save(&snapshot, &doc).unwrap();
                untouched.push((snapshot.clone(), fs::read(&snapshot).unwrap()));
            }
        }
        let (snapshot, metadata) = store.snapshot_paths(1, 5).unwrap();
        let mut info = make_metadata(1, 5, "A".into(), None, 0, &doc);
        info.saved_at_nanos = 10;
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        let stored: SnapshotMeta = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        assert_eq!(stored.saved_at_nanos, 10);
        assert_eq!(generation_metadata(folder, 1).len(), 1);
        for (path, bytes) in untouched {
            assert_eq!(
                fs::read(&path).unwrap(),
                bytes,
                "changed {}",
                path.display()
            );
        }
        store.finish_cleanly();
    }

    #[test]
    fn recovery_rejects_mismatched_writer_identity_before_changing_files() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        let (snapshot, metadata) = store.snapshot_paths(1, 5).unwrap();
        let info = make_metadata(1, 5, "A".into(), None, 0, &doc);
        write_snapshot(&snapshot, &metadata, &doc, &info).unwrap();
        let snapshot_bytes = fs::read(&snapshot).unwrap();
        let metadata_bytes = fs::read(&metadata).unwrap();
        for field in 0..4 {
            let mut invalid = info.clone();
            match field {
                0 => invalid.version = 2,
                1 => invalid.document_id = 2,
                2 => invalid.state_token = 6,
                _ => invalid.width = 7,
            }
            assert!(write_snapshot(&snapshot, &metadata, &doc, &invalid).is_err());
            assert_eq!(fs::read(&snapshot).unwrap(), snapshot_bytes);
            assert_eq!(fs::read(&metadata).unwrap(), metadata_bytes);
        }
        let (other_snapshot, _) = store.snapshot_paths(2, 5).unwrap();
        assert!(write_snapshot(&other_snapshot, &metadata, &doc, &info).is_err());
        assert!(!other_snapshot.exists());
        store.finish_cleanly();
    }

    #[test]
    #[cfg(windows)]
    fn recovery_order_read_failure_keeps_previous_files_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        let (old, old_metadata) = store.snapshot_paths(1, 5).unwrap();
        let info = make_metadata(1, 5, "A".into(), None, 0, &doc);
        write_snapshot(&old, &old_metadata, &doc, &info).unwrap();
        let old_bytes = fs::read(&old).unwrap();
        let metadata_bytes = fs::read(&old_metadata).unwrap();
        let lock = exclusive_open(&old_metadata).unwrap();
        let (new, new_metadata) = store.snapshot_paths(1, 6).unwrap();
        let info = make_metadata(1, 6, "A".into(), None, 0, &doc);
        assert!(write_snapshot(&new, &new_metadata, &doc, &info).is_err());
        assert!(!new.exists());
        assert!(!new_metadata.exists());
        assert_eq!(fs::read(&old).unwrap(), old_bytes);
        drop(lock);
        assert_eq!(fs::read(&old_metadata).unwrap(), metadata_bytes);
        write_snapshot(&new, &new_metadata, &doc, &info).unwrap();
        assert!(new.exists());
        assert!(old.exists());
        store.finish_cleanly();
    }

    #[test]
    fn recovery_keeps_two_complete_generations_per_tab() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new_in(root.path().to_path_buf());
        let mut doc = Document::new(8, 8);
        for token in 1..=3 {
            doc.layers[0]
                .pixels
                .set_pixel(0, 0, [token as u8, 0, 0, 255]);
            let (snapshot, metadata) = store.snapshot_paths(1, token).unwrap();
            write_snapshot(
                &snapshot,
                &metadata,
                &doc,
                &make_metadata(1, token, "A".into(), None, 0, &doc),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            generation_metadata(store.current_dir().unwrap(), 1).len(),
            2
        );
        assert!(!store.snapshot_paths(1, 1).unwrap().0.exists());
        store.finish_cleanly();
    }

    #[test]
    fn automatic_recovery_snapshots_include_every_untitled_tab() {
        let root = tempfile::tempdir().unwrap();
        let mut app = EfudeApp::default();
        app.recovery.finish_cleanly();
        app.recovery = Store::new_in(root.path().to_path_buf());
        app.doc = Document::new(8, 8);
        app.history.begin();
        app.history.record_pixel(&app.doc.layers[0], 0);
        app.doc.layers[0].pixels.set_pixel(0, 0, [10, 20, 30, 255]);
        app.history.commit();
        let first_id = app.history.document_id();
        app.open_document_tab();
        app.doc = Document::new(8, 8);
        app.history.begin();
        app.history.record_pixel(&app.doc.layers[0], 0);
        app.doc.layers[0].pixels.set_pixel(0, 0, [40, 50, 60, 255]);
        app.history.commit();
        let second_id = app.history.document_id();
        let ctx = egui::Context::default();
        assert_eq!(app.queue_recovery_snapshots(&ctx).unwrap(), 2);
        let _ = app.io_task_sender.send(IoTask::Shutdown);
        app.io_worker.take().unwrap().join().unwrap();
        for (id, expected) in [
            (first_id, [10, 20, 30, 255]),
            (second_id, [40, 50, 60, 255]),
        ] {
            let info_token = if id == first_id {
                app.tabs.slots[0]
                    .parked
                    .as_ref()
                    .unwrap()
                    .history
                    .state_token()
            } else {
                app.history.state_token()
            };
            let (snapshot, metadata) = app.recovery.snapshot_paths(id, info_token).unwrap();
            assert_eq!(
                efude_io::load(&snapshot).unwrap().layers[0]
                    .pixels
                    .pixel(0, 0),
                expected
            );
            let info: SnapshotMeta = serde_json::from_slice(&fs::read(metadata).unwrap()).unwrap();
            assert!(info.original_path.is_none());
        }
        app.recovery._lease.take();
        app.recovery.current = None; // Keep the snapshots as a crashed app would.
        drop(app);
        let mut restarted = Store::new_in(root.path().to_path_buf());
        assert_eq!(restarted.candidates.len(), 2);
        restarted.finish_cleanly();
    }

    #[test]
    fn restored_work_opens_as_unsaved_tab_without_overwriting_the_original() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.efude");
        efude_io::save(&original, &Document::new(8, 8)).unwrap();
        let mut old = Store::new_in(root.path().to_path_buf());
        let mut changed = Document::new(8, 8);
        changed.layers[0].pixels.set_pixel(0, 0, [77, 88, 99, 255]);
        let (snapshot, metadata_path) = old.snapshot_paths(7, 8).unwrap();
        let info = make_metadata(
            7,
            8,
            "original.efude".into(),
            Some(original.clone()),
            0,
            &changed,
        );
        write_snapshot(&snapshot, &metadata_path, &changed, &info).unwrap();
        old._lease.take();

        let mut app = EfudeApp::default();
        app.recovery.finish_cleanly();
        app.recovery = Store::new_in(root.path().to_path_buf());
        assert_eq!(app.recovery.candidates.len(), 1);
        let ctx = egui::Context::default();
        app.io_task_sender
            .send(IoTask::LoadRecovery {
                snapshot: snapshot.clone(),
                metadata: metadata_path,
                info,
                repaint: ctx.clone(),
            })
            .unwrap();
        for _ in 0..100 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.update_ui(ctx));
            if app.history.is_dirty() && app.doc.layers[0].pixels.pixel(0, 0) == [77, 88, 99, 255] {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.history.is_dirty());
        assert!(app.doc_path.is_none());
        assert_eq!(app.suggested_save_name(), "original (復元).efude");
        assert_eq!(
            efude_io::load(&original).unwrap().layers[0]
                .pixels
                .pixel(0, 0),
            [0; 4]
        );
        drop(app);
        assert!(!snapshot.exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_live_second_instance_is_not_mistaken_for_a_crash() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Store::new_in(root.path().to_path_buf());
        let doc = Document::new(8, 8);
        let (snapshot, metadata_path) = first.snapshot_paths(1, 2).unwrap();
        write_snapshot(
            &snapshot,
            &metadata_path,
            &doc,
            &make_metadata(1, 2, "live".into(), None, 0, &doc),
        )
        .unwrap();
        let mut second = Store::new_in(root.path().to_path_buf());
        assert!(second.candidates.is_empty());
        second.finish_cleanly();
        first.finish_cleanly();
    }
}
