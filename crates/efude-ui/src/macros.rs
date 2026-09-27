// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Small, semantic document macros. Pointer events and brush strokes are not replayed.
use super::*;
use std::{fs, io::Write, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Step {
    NewRaster,
    DuplicateActive,
    SetName { name: String },
    SetOpacity { value: f32 },
    SetVisibility { visible: bool },
    SetBlend { mode: BlendMode },
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Definition {
    pub version: u32,
    pub name: String,
    pub steps: Vec<Step>,
}

pub(crate) struct Saved {
    pub path: PathBuf,
    pub definition: Definition,
}

pub(crate) struct Recording {
    pub document_id: u64,
    pub steps: Vec<Step>,
}

fn directory() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("Efude-neo")
        .join("macros")
}

fn validate(definition: &Definition) -> Result<(), String> {
    if definition.version != 1
        || definition.name.trim().is_empty()
        || definition.name.chars().count() > 120
        || definition.steps.is_empty()
        || definition.steps.len() > 200
    {
        return Err("マクロの形式・名前・手順数が不正です".into());
    }
    for step in &definition.steps {
        match step {
            Step::SetOpacity { value } if !value.is_finite() || !(0.0..=1.0).contains(value) => {
                return Err("不透明度が不正です".into());
            }
            Step::SetName { name } if name.chars().count() > 200 => {
                return Err("レイヤー名が長すぎます".into());
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn load_saved() -> Vec<Saved> {
    let mut saved = Vec::new();
    let Ok(items) = fs::read_dir(directory()) else {
        return saved;
    };
    for item in items.flatten() {
        let path = item.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".efmacro.json"))
        {
            continue;
        }
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 128 * 1024)
        {
            continue;
        }
        if let Ok(bytes) = fs::read(&path)
            && let Ok(definition) = serde_json::from_slice::<Definition>(&bytes)
            && validate(&definition).is_ok()
        {
            saved.push(Saved { path, definition });
        }
    }
    saved.sort_by(|a, b| a.definition.name.cmp(&b.definition.name));
    saved
}

fn save(definition: Definition) -> Result<Saved, String> {
    validate(&definition)?;
    let directory = directory();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let filename: String = definition
        .name
        .chars()
        .filter(|ch| !"/\\:*?\"<>|".contains(*ch) && !ch.is_control())
        .collect();
    let filename = filename.trim().trim_matches('.');
    if filename.is_empty() {
        return Err("マクロ名に使える文字がありません".into());
    }
    let bytes = serde_json::to_vec_pretty(&definition).map_err(|error| error.to_string())?;
    let mut temp =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    temp.write_all(&bytes).map_err(|error| error.to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    for attempt in 0..100 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let path = directory.join(format!("{filename}{suffix}.efmacro.json"));
        match temp.persist_noclobber(&path) {
            Ok(_) => {
                return Ok(Saved { path, definition });
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = error.file;
            }
            Err(error) => return Err(error.error.to_string()),
        }
    }
    Err("同じ名前のマクロが多すぎます".into())
}

fn can_coalesce(previous: &Step, next: &Step) -> bool {
    matches!(
        (previous, next),
        (Step::SetName { .. }, Step::SetName { .. })
            | (Step::SetOpacity { .. }, Step::SetOpacity { .. })
            | (Step::SetVisibility { .. }, Step::SetVisibility { .. })
            | (Step::SetBlend { .. }, Step::SetBlend { .. })
    )
}

impl EfudeApp {
    pub(crate) fn record_macro_step(&mut self, step: Step) {
        if self.macro_replaying {
            return;
        }
        let Some(recording) = self.macro_recording.as_mut() else {
            return;
        };
        if recording.document_id != self.history.document_id() {
            return;
        }
        if recording
            .steps
            .last()
            .is_some_and(|previous| can_coalesce(previous, &step))
        {
            recording.steps.pop();
        }
        if recording.steps.len() < 200 {
            recording.steps.push(step);
        } else {
            self.status = "マクロは200手順までです".into();
        }
    }

    pub(crate) fn macro_menu_ui(&mut self, ui: &mut egui::Ui) {
        match &self.macro_recording {
            None => {
                if ui
                    .button(self.text("マクロの記録を開始", "Start macro recording"))
                    .clicked()
                {
                    self.commit_pending_guide_edit();
                    self.macro_recording = Some(Recording {
                        document_id: self.history.document_id(),
                        steps: Vec::new(),
                    });
                    self.status = self
                        .text(
                            "レイヤーの作成・複製・プロパティ変更を記録します",
                            "Recording layer creation, duplication and properties",
                        )
                        .into();
                    ui.close_menu();
                }
            }
            Some(recording) if recording.document_id == self.history.document_id() => {
                if ui
                    .button(self.text("マクロの記録を終了して保存…", "Finish and save macro…"))
                    .clicked()
                {
                    self.macro_name = String::from("レイヤー手順");
                    self.show_macro_save = true;
                    ui.close_menu();
                }
                if ui
                    .button(self.text("記録を破棄", "Discard recording"))
                    .clicked()
                {
                    self.macro_recording = None;
                    ui.close_menu();
                }
            }
            Some(_) => {
                ui.label(self.text(
                    "別のタブでマクロを記録中",
                    "Recording a macro in another tab",
                ));
            }
        }
        ui.separator();
        if self.saved_macros.is_empty() {
            ui.label(self.text("保存済みマクロはありません", "No saved macros"));
        } else {
            let mut chosen = None;
            ui.menu_button(self.text("マクロを実行", "Run macro"), |ui| {
                for (index, saved) in self.saved_macros.iter().enumerate() {
                    if ui.button(&saved.definition.name).clicked() {
                        chosen = Some(index);
                        ui.close_menu();
                    }
                }
            });
            if let Some(index) = chosen {
                let definition = self.saved_macros[index].definition.clone();
                self.status = match self.run_macro(&definition) {
                    Ok(()) => format!("マクロを実行しました: {}", definition.name),
                    Err(error) => format!("マクロを実行できません: {error}"),
                };
            }
        }
        ui.label(self.text(
            "記録対象: レイヤー作成・複製・名前・表示・不透明度・合成モード",
            "Records: layer creation, duplication, name, visibility, opacity and blend mode",
        ));
    }

    pub(crate) fn macro_save_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_macro_save {
            return;
        }
        let mut open = self.show_macro_save;
        egui::Window::new(self.text("マクロを保存", "Save macro"))
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(self.text("マクロ名", "Macro name"));
                ui.text_edit_singleline(&mut self.macro_name);
                let count = self
                    .macro_recording
                    .as_ref()
                    .map_or(0, |recording| recording.steps.len());
                ui.label(format!("{count} {}", self.text("手順", "steps")));
                if ui
                    .add_enabled(
                        count > 0 && !self.macro_name.trim().is_empty(),
                        egui::Button::new(self.text("保存", "Save")),
                    )
                    .clicked()
                {
                    let definition = Definition {
                        version: 1,
                        name: self.macro_name.trim().into(),
                        steps: self.macro_recording.as_ref().unwrap().steps.clone(),
                    };
                    match save(definition) {
                        Ok(saved) => {
                            self.status = format!("マクロを保存しました: {}", saved.path.display());
                            self.saved_macros.push(saved);
                            self.saved_macros
                                .sort_by(|a, b| a.definition.name.cmp(&b.definition.name));
                            self.macro_recording = None;
                            self.show_macro_save = false;
                        }
                        Err(error) => self.status = error,
                    }
                }
            });
        if self.show_macro_save {
            self.show_macro_save = open;
        }
    }

    pub(crate) fn macro_status(&self) -> Option<String> {
        let recording = self.macro_recording.as_ref()?;
        if recording.document_id != self.history.document_id() {
            return None;
        }
        Some(format!(
            "● {} · {} {}",
            self.text("マクロ記録中", "Macro recording"),
            recording.steps.len(),
            self.text("手順", "steps")
        ))
    }

    fn run_macro(&mut self, definition: &Definition) -> Result<(), String> {
        validate(definition)?;
        if self.macro_recording.is_some() {
            return Err("記録中のマクロは実行できません".into());
        }
        if self.history.is_active() || !self.active.is_empty() || self.filter_pending {
            return Err("編集中の操作を確定してから実行してください".into());
        }
        self.commit_pending_guide_edit();
        let selected_before = self.selected_layer;
        self.history.begin();
        self.macro_replaying = true;
        let mut result = Ok(());
        for (index, step) in definition.steps.iter().enumerate() {
            if let Err(error) = self.apply_macro_step(step) {
                result = Err(format!("{}手目: {error}", index + 1));
                break;
            }
        }
        self.macro_replaying = false;
        if result.is_ok() {
            self.history.commit();
        } else {
            self.history.cancel(&mut self.doc);
            self.selected_layer = selected_before;
        }
        self.canvas_texture_dirty = true;
        self.navigator_texture_dirty = true;
        result
    }

    fn apply_macro_step(&mut self, step: &Step) -> Result<(), String> {
        if self.selected_layer >= self.doc.layers.len() {
            return Err("対象レイヤーがありません".into());
        }
        match step {
            Step::NewRaster => {
                if self.doc.layers.len() >= 2000
                    || self.doc.layers.iter().any(|layer| layer.id == u64::MAX)
                {
                    return Err("レイヤー数またはIDの上限です".into());
                }
                self.add_raster_layer();
            }
            Step::DuplicateActive => {
                let id = self.doc.layers[self.selected_layer].id;
                let copied = efude_canvas::subtree_ids(&self.doc.layers, id).len();
                if self.doc.layers.len() + copied > 2000
                    || self
                        .doc
                        .layers
                        .iter()
                        .any(|layer| layer.id > u64::MAX - copied as u64)
                {
                    return Err("レイヤー数またはIDの上限です".into());
                }
                self.duplicate_layer_subtree();
            }
            Step::SetName { name } => {
                let layer = &mut self.doc.layers[self.selected_layer];
                let before = layer.property_state();
                layer.name = name.clone();
                let after = layer.property_state();
                self.history
                    .record_layer_properties(layer.id, before, after);
            }
            Step::SetOpacity { value } => {
                let layer = &mut self.doc.layers[self.selected_layer];
                let before = layer.property_state();
                layer.opacity = *value;
                let after = layer.property_state();
                self.history
                    .record_layer_properties(layer.id, before, after);
            }
            Step::SetVisibility { visible } => {
                let layer = &mut self.doc.layers[self.selected_layer];
                let before = layer.property_state();
                layer.visible = *visible;
                let after = layer.property_state();
                self.history
                    .record_layer_properties(layer.id, before, after);
            }
            Step::SetBlend { mode } => {
                let layer = &mut self.doc.layers[self.selected_layer];
                let before = layer.property_state();
                layer.blend = *mode;
                let after = layer.property_state();
                self.history
                    .record_layer_properties(layer.id, before, after);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_replays_on_another_document_as_one_undo_step() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(32, 32);
        let definition = Definition {
            version: 1,
            name: "Setup".into(),
            steps: vec![
                Step::NewRaster,
                Step::SetName { name: "Ink".into() },
                Step::SetOpacity { value: 0.7 },
            ],
        };
        app.run_macro(&definition).unwrap();
        assert_eq!(app.doc.layers.len(), 2);
        assert_eq!(app.doc.layers[1].name, "Ink");
        assert_eq!(app.doc.layers[1].opacity, 0.7);
        app.history.undo_document(&mut app.doc);
        assert_eq!(app.doc.layers.len(), 1);
    }

    #[test]
    fn failed_macro_rolls_back_every_step() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.doc.layers = (1..=1999)
            .map(|id| efude_canvas::Layer::new(id, "L", 8, 8))
            .collect();
        let state = app.history.state_token();
        let definition = Definition {
            version: 1,
            name: "Too many".into(),
            steps: vec![Step::NewRaster, Step::NewRaster],
        };
        assert!(app.run_macro(&definition).is_err());
        assert_eq!(app.doc.layers.len(), 1999);
        assert_eq!(app.history.state_token(), state);
        assert!(!app.history.can_undo());
    }

    #[test]
    fn normal_layer_buttons_are_recorded_as_semantic_steps() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.macro_recording = Some(Recording {
            document_id: app.history.document_id(),
            steps: Vec::new(),
        });
        app.add_raster_layer();
        app.duplicate_layer_subtree();
        assert!(matches!(
            app.macro_recording.as_ref().unwrap().steps.as_slice(),
            [Step::NewRaster, Step::DuplicateActive]
        ));
    }
}
