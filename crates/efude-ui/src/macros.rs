// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Small, semantic document macros. Pointer events and brush strokes are not replayed.
use super::*;
use std::collections::{HashMap, HashSet};
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Target {
    #[default]
    Current,
    Start,
    External {
        key: u64,
        name: String,
    },
    Created {
        step_id: u32,
        #[serde(default)]
        offset: usize,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Command {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub target: Target,
    #[serde(flatten)]
    pub step: Step,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Definition {
    pub version: u32,
    pub name: String,
    pub steps: Vec<Command>,
}

pub(crate) struct Saved {
    pub path: PathBuf,
    pub definition: Definition,
}

pub(crate) struct Recording {
    pub document_id: u64,
    pub steps: Vec<Command>,
    targets: HashMap<u64, Target>,
}

impl Recording {
    fn new(app: &EfudeApp) -> Self {
        let start = app.doc.layers.get(app.selected_layer).map(|layer| layer.id);
        Self {
            document_id: app.history.document_id(),
            steps: Vec::new(),
            targets: app
                .doc
                .layers
                .iter()
                .map(|layer| {
                    (
                        layer.id,
                        if Some(layer.id) == start {
                            Target::Start
                        } else {
                            Target::External {
                                key: layer.id,
                                name: layer.name.clone(),
                            }
                        },
                    )
                })
                .collect(),
        }
    }
}

pub(crate) struct Editor {
    definition: Definition,
    path: Option<PathBuf>,
    recording: bool,
    error: String,
    next_id: u32,
    target_keys: HashMap<(u64, u64), u64>,
}

pub(crate) struct RunDialog {
    definition: Definition,
    document_id: u64,
    start: u64,
    bindings: HashMap<u64, u64>,
    error: String,
}

fn normalize(mut definition: Definition) -> Definition {
    if definition.version == 1 {
        definition.version = 2;
        for (index, command) in definition.steps.iter_mut().enumerate() {
            command.id = index as u32 + 1;
            command.target = Target::Current;
        }
    }
    definition
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
    if definition.version != 2
        || definition.name.trim().is_empty()
        || definition.name.chars().count() > 120
        || definition.steps.is_empty()
        || definition.steps.len() > 200
    {
        return Err("マクロの形式・名前・手順数が不正です".into());
    }
    let mut ids = HashSet::new();
    let mut generators = HashMap::new();
    let mut external_names = HashMap::new();
    for command in &definition.steps {
        if command.id == 0 || !ids.insert(command.id) {
            return Err("手順IDが不正または重複しています".into());
        }
        match &command.target {
            Target::Created { step_id, offset }
                if !generators.contains_key(step_id)
                    || *offset >= 2000
                    || (*offset > 0 && generators.get(step_id) == Some(&false)) =>
            {
                return Err(format!(
                    "手順{}の対象が、まだ作成されていないレイヤーを参照しています",
                    command.id
                ));
            }
            Target::External { key, name } => {
                if *key == 0 || name.chars().count() > 200 {
                    return Err("対象レイヤーの指定が不正です".into());
                }
                if external_names
                    .insert(*key, name)
                    .is_some_and(|before| before != name)
                {
                    return Err("同じ対象キーに異なる名前が指定されています".into());
                }
            }
            _ => {}
        }
        match &command.step {
            Step::SetOpacity { value } if !value.is_finite() || !(0.0..=1.0).contains(value) => {
                return Err("不透明度が不正です".into());
            }
            Step::SetName { name } if name.chars().count() > 200 => {
                return Err("レイヤー名が長すぎます".into());
            }
            _ => {}
        }
        if matches!(command.step, Step::NewRaster | Step::DuplicateActive) {
            generators.insert(command.id, matches!(command.step, Step::DuplicateActive));
        }
    }
    Ok(())
}

fn editor_targets(editor: &mut Editor, doc: &Document, document_id: u64) -> Vec<(Target, String)> {
    let existing = external_targets(&editor.definition);
    let mut next_key = existing
        .iter()
        .map(|(key, _)| *key)
        .chain(editor.target_keys.values().copied())
        .max()
        .unwrap_or(0);
    doc.layers
        .iter()
        .map(|layer| {
            let identity = (document_id, layer.id);
            if editor.target_keys.get(&identity).is_some_and(|key| {
                existing
                    .iter()
                    .any(|(old_key, name)| old_key == key && *name != layer.name)
            }) {
                editor.target_keys.remove(&identity);
            }
            let key = *editor.target_keys.entry(identity).or_insert_with(|| {
                // Keys in a saved macro belong to the macro, not the current document.
                // Reuse an unambiguous name; allocate distinct keys for all other layers.
                let matching: Vec<_> = existing
                    .iter()
                    .filter(|(_, name)| *name == layer.name)
                    .collect();
                if matching.len() == 1
                    && doc.layers.iter().filter(|l| l.name == layer.name).count() == 1
                {
                    matching[0].0
                } else {
                    next_key = next_key.checked_add(1).unwrap_or(0);
                    next_key
                }
            });
            (
                Target::External {
                    key,
                    name: layer.name.clone(),
                },
                format!("{} · #{}", layer.name, layer.id),
            )
        })
        .collect()
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
        {
            let definition = normalize(definition);
            if validate(&definition).is_ok() {
                saved.push(Saved { path, definition });
            }
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

fn external_targets(definition: &Definition) -> Vec<(u64, String)> {
    let mut seen = HashSet::new();
    definition
        .steps
        .iter()
        .filter_map(|command| {
            if let Target::External { key, name } = &command.target
                && seen.insert(*key)
            {
                Some((*key, name.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn suggested_bindings(definition: &Definition, doc: &Document) -> HashMap<u64, u64> {
    external_targets(definition)
        .into_iter()
        .filter_map(|(key, name)| {
            let matches: Vec<_> = doc
                .layers
                .iter()
                .filter(|layer| layer.name == name)
                .collect();
            (matches.len() == 1).then(|| (key, matches[0].id))
        })
        .collect()
}

fn validate_bindings(
    definition: &Definition,
    doc: &Document,
    start: u64,
    bindings: &HashMap<u64, u64>,
) -> Result<(), String> {
    if !doc.layers.iter().any(|layer| layer.id == start) {
        return Err("開始レイヤーがありません".into());
    }
    for (key, name) in external_targets(definition) {
        if !bindings
            .get(&key)
            .is_some_and(|id| doc.layers.iter().any(|layer| layer.id == *id))
        {
            return Err(format!("対象「{name}」のレイヤーを選択してください"));
        }
    }
    Ok(())
}

fn step_label(step: &Step, english: bool) -> &'static str {
    match (step, english) {
        (Step::NewRaster, false) => "レイヤー作成",
        (Step::NewRaster, true) => "New layer",
        (Step::DuplicateActive, false) => "レイヤー複製",
        (Step::DuplicateActive, true) => "Duplicate layer",
        (Step::SetName { .. }, false) => "名前",
        (Step::SetName { .. }, true) => "Name",
        (Step::SetOpacity { .. }, false) => "不透明度",
        (Step::SetOpacity { .. }, true) => "Opacity",
        (Step::SetVisibility { .. }, false) => "表示",
        (Step::SetVisibility { .. }, true) => "Visibility",
        (Step::SetBlend { .. }, false) => "合成モード",
        (Step::SetBlend { .. }, true) => "Blend mode",
    }
}

fn target_label(target: &Target, english: bool) -> String {
    match target {
        Target::Current => if english {
            "Current selection"
        } else {
            "その時点の選択レイヤー"
        }
        .into(),
        Target::Start => if english {
            "Selected at start"
        } else {
            "実行開始時のレイヤー"
        }
        .into(),
        Target::External { name, .. } => name.clone(),
        Target::Created { step_id, offset } => {
            if english {
                format!("Created by #{step_id} ({} )", offset + 1)
            } else {
                format!("手順#{step_id}で作成（{}）", offset + 1)
            }
        }
    }
}

fn save_edited(definition: &Definition, path: &std::path::Path) -> Result<(), String> {
    validate(definition)?;
    let bytes = serde_json::to_vec_pretty(definition).map_err(|e| e.to_string())?;
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap_or(std::path::Path::new(".")))
            .map_err(|e| e.to_string())?;
    temp.write_all(&bytes).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn layer_picker(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash,
    doc: &Document,
    selected: &mut u64,
    missing: &str,
) {
    let label = doc
        .layers
        .iter()
        .find(|layer| layer.id == *selected)
        .map_or(missing, |layer| layer.name.as_str());
    egui::ComboBox::from_id_salt(salt)
        .selected_text(label)
        .show_ui(ui, |ui| {
            for layer in &doc.layers {
                ui.selectable_value(
                    selected,
                    layer.id,
                    format!("{} · #{}", layer.name, layer.id),
                );
            }
        });
}

impl EfudeApp {
    pub(crate) fn macro_editor_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.macro_editor.take() else {
            return;
        };
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut save_now = false;
        let mut save_copy = false;
        let mut change = None;
        let external_choices = editor_targets(&mut editor, &self.doc, self.history.document_id());
        egui::Window::new(t("マクロの手順を編集", "Edit macro steps"))
            .open(&mut open)
            .default_width(590.0)
            .show(ctx, |ui| {
                ui.text_edit_singleline(&mut editor.definition.name);
                egui::ScrollArea::vertical()
                    .max_height(390.0)
                    .id_salt("macro-editor-steps")
                    .show(ui, |ui| {
                        let generator_choices: Vec<_> = editor
                            .definition
                            .steps
                            .iter()
                            .filter(|c| matches!(c.step, Step::NewRaster | Step::DuplicateActive))
                            .map(|c| (c.id, step_label(&c.step, english)))
                            .collect();
                        for (index, command) in editor.definition.steps.iter_mut().enumerate() {
                            ui.push_id(command.id, |ui| {
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(format!("{} · #{}", index + 1, command.id));
                                    egui::ComboBox::from_id_salt("operation")
                                        .selected_text(step_label(&command.step, english))
                                        .show_ui(ui, |ui| {
                                            for option in [
                                                Step::NewRaster,
                                                Step::DuplicateActive,
                                                Step::SetName {
                                                    name: "Layer".into(),
                                                },
                                                Step::SetOpacity { value: 1.0 },
                                                Step::SetVisibility { visible: true },
                                                Step::SetBlend {
                                                    mode: BlendMode::Normal,
                                                },
                                            ] {
                                                if ui
                                                    .selectable_label(
                                                        std::mem::discriminant(&command.step)
                                                            == std::mem::discriminant(&option),
                                                        step_label(&option, english),
                                                    )
                                                    .clicked()
                                                {
                                                    command.step = option;
                                                }
                                            }
                                        });
                                    if ui
                                        .add_enabled(index > 0, egui::Button::new("↑").small())
                                        .clicked()
                                    {
                                        change = Some((index, -1));
                                    }
                                    if ui.small_button("↓").clicked() {
                                        change = Some((index, 1));
                                    }
                                    if ui.small_button(t("削除", "Delete")).clicked() {
                                        change = Some((index, 0));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label(t("対象", "Target"));
                                    egui::ComboBox::from_id_salt("target")
                                        .selected_text(target_label(&command.target, english))
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut command.target,
                                                Target::Start,
                                                t("実行開始時のレイヤー", "Selected at start"),
                                            );
                                            ui.selectable_value(
                                                &mut command.target,
                                                Target::Current,
                                                t("その時点の選択レイヤー", "Current selection"),
                                            );
                                            for (target, label) in &external_choices {
                                                ui.selectable_value(
                                                    &mut command.target,
                                                    target.clone(),
                                                    label,
                                                );
                                            }
                                            for &(step_id, label) in &generator_choices {
                                                ui.selectable_value(
                                                    &mut command.target,
                                                    Target::Created { step_id, offset: 0 },
                                                    format!("#{step_id} · {label}"),
                                                );
                                            }
                                        });
                                    if let Target::Created { offset, .. } = &mut command.target {
                                        ui.label(t("複製した中の順番", "Created layer position"));
                                        ui.add(egui::DragValue::new(offset).range(0..=1999));
                                    }
                                });
                                match &mut command.step {
                                    Step::SetName { name } => {
                                        ui.text_edit_singleline(name);
                                    }
                                    Step::SetOpacity { value } => {
                                        ui.add(
                                            egui::Slider::new(value, 0.0..=1.0)
                                                .text(t("不透明度", "Opacity")),
                                        );
                                    }
                                    Step::SetVisibility { visible } => {
                                        ui.checkbox(visible, t("表示する", "Visible"));
                                    }
                                    Step::SetBlend { mode } => {
                                        egui::ComboBox::from_id_salt("blend")
                                            .selected_text(format!("{mode:?}"))
                                            .show_ui(ui, |ui| {
                                                for option in [
                                                    BlendMode::Normal,
                                                    BlendMode::Multiply,
                                                    BlendMode::Screen,
                                                    BlendMode::Overlay,
                                                    BlendMode::Darken,
                                                    BlendMode::Lighten,
                                                    BlendMode::ColorDodge,
                                                    BlendMode::ColorBurn,
                                                    BlendMode::HardLight,
                                                    BlendMode::SoftLight,
                                                    BlendMode::Difference,
                                                    BlendMode::Exclusion,
                                                    BlendMode::Add,
                                                    BlendMode::Subtract,
                                                ] {
                                                    ui.selectable_value(
                                                        mode,
                                                        option,
                                                        format!("{option:?}"),
                                                    );
                                                }
                                            });
                                    }
                                    _ => {}
                                }
                            });
                        }
                    });
                if ui
                    .add_enabled(
                        editor.definition.steps.len() < 200,
                        egui::Button::new(t("手順を追加", "Add step")),
                    )
                    .clicked()
                {
                    let id = editor.next_id;
                    editor.next_id = editor.next_id.saturating_add(1);
                    editor.definition.steps.push(Command {
                        id,
                        target: Target::Start,
                        step: Step::NewRaster,
                    });
                }
                if let Err(error) = validate(&editor.definition) {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                if !editor.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &editor.error);
                }
                ui.horizontal(|ui| {
                    let valid = validate(&editor.definition).is_ok();
                    save_now = ui
                        .add_enabled(valid, egui::Button::new(t("保存", "Save")))
                        .clicked();
                    if editor.path.is_some() {
                        save_copy = ui
                            .add_enabled(
                                valid,
                                egui::Button::new(t(
                                    "別のマクロとして保存",
                                    "Save as another macro",
                                )),
                            )
                            .clicked();
                    }
                });
            });
        if let Some((index, delta)) = change {
            if delta == 0 {
                editor.definition.steps.remove(index);
            } else {
                let target = index as isize + delta;
                if target >= 0 && target < editor.definition.steps.len() as isize {
                    editor.definition.steps.swap(index, target as usize);
                }
            }
        }
        if save_now || save_copy {
            let result = if !save_copy && let Some(path) = &editor.path {
                save_edited(&editor.definition, path).map(|()| Saved {
                    path: path.clone(),
                    definition: editor.definition.clone(),
                })
            } else {
                save(editor.definition.clone())
            };
            match result {
                Ok(saved) => {
                    let path = saved.path.clone();
                    self.saved_macros.retain(|previous| previous.path != path);
                    self.saved_macros.push(saved);
                    self.saved_macros
                        .sort_by(|a, b| a.definition.name.cmp(&b.definition.name));
                    if editor.recording {
                        self.macro_recording = None;
                    }
                    self.status = t("マクロを保存しました", "Macro saved").into();
                    open = false;
                }
                Err(error) => editor.error = error,
            }
        }
        if open {
            self.macro_editor = Some(editor);
        }
    }

    fn open_macro_run(&mut self, definition: Definition) {
        if self.macro_recording.is_some() {
            self.status = "記録中のマクロは実行できません".into();
            return;
        }
        if let Some(layer) = self.doc.layers.get(self.selected_layer) {
            self.macro_run = Some(RunDialog {
                bindings: suggested_bindings(&definition, &self.doc),
                definition,
                document_id: self.history.document_id(),
                start: layer.id,
                error: String::new(),
            });
        }
    }

    pub(crate) fn macro_run_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.macro_run.take() else {
            return;
        };
        if dialog.document_id != self.history.document_id() {
            self.status = "実行する文書が変わりました。マクロを選び直してください".into();
            return;
        }
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut run = false;
        egui::Window::new(t("マクロの対象を確認", "Confirm macro targets"))
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} · {} {}",
                    dialog.definition.name,
                    dialog.definition.steps.len(),
                    t("手順", "steps")
                ));
                ui.label(t(
                    "実行全体を1回の「元に戻す」で戻せます。",
                    "The whole macro can be undone in one step.",
                ));
                ui.horizontal(|ui| {
                    ui.label(t("開始時のレイヤー", "Starting layer"));
                    layer_picker(
                        ui,
                        "macro-start",
                        &self.doc,
                        &mut dialog.start,
                        t("選択してください", "Choose a layer"),
                    );
                });
                for (key, name) in external_targets(&dialog.definition) {
                    let mut bound = dialog.bindings.get(&key).copied().unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label(&name);
                        layer_picker(
                            ui,
                            ("macro-external", key),
                            &self.doc,
                            &mut bound,
                            t("選択してください", "Choose a layer"),
                        );
                    });
                    if bound != 0 {
                        dialog.bindings.insert(key, bound);
                    }
                }
                let valid = validate(&dialog.definition).and_then(|()| {
                    validate_bindings(
                        &dialog.definition,
                        &self.doc,
                        dialog.start,
                        &dialog.bindings,
                    )
                });
                if let Err(error) = &valid {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                if !dialog.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &dialog.error);
                }
                run = ui
                    .add_enabled(valid.is_ok(), egui::Button::new(t("実行", "Run")))
                    .clicked();
            });
        if run {
            match self.run_macro_bound(&dialog.definition, dialog.start, &dialog.bindings) {
                Ok(()) => {
                    self.status = format!(
                        "{}: {}",
                        t("マクロを実行しました", "Macro completed"),
                        dialog.definition.name
                    );
                    open = false;
                }
                Err(error) => dialog.error = error,
            }
        }
        if open {
            self.macro_run = Some(dialog);
        }
    }
    pub(crate) fn record_macro_step(&mut self, step: Step, layer_id: u64) {
        if self.macro_replaying
            || self
                .macro_editor
                .as_ref()
                .is_some_and(|editor| editor.recording)
        {
            return;
        }
        let Some(recording) = self.macro_recording.as_mut() else {
            return;
        };
        if recording.document_id != self.history.document_id() {
            return;
        }
        let target = recording
            .targets
            .get(&layer_id)
            .cloned()
            .unwrap_or_else(|| {
                let target = Target::External {
                    key: layer_id,
                    name: self
                        .doc
                        .layers
                        .iter()
                        .find(|l| l.id == layer_id)
                        .map_or(String::new(), |l| l.name.clone()),
                };
                recording.targets.insert(layer_id, target.clone());
                target
            });
        let mut id = recording.steps.last().map_or(1, |command| command.id + 1);
        if recording.steps.last().is_some_and(|previous| {
            previous.target == target && can_coalesce(&previous.step, &step)
        }) {
            id = recording.steps.pop().unwrap().id;
        }
        if recording.steps.len() < 200 {
            recording.steps.push(Command { id, target, step });
        } else {
            self.status = "マクロは200手順までです".into();
        }
    }

    pub(crate) fn record_macro_creation(&mut self, step: Step, source_id: u64, created: &[u64]) {
        if self.macro_replaying {
            return;
        }
        let before = self
            .macro_recording
            .as_ref()
            .map_or(0, |recording| recording.steps.len());
        self.record_macro_step(step, source_id);
        if let Some(recording) = &mut self.macro_recording
            && recording.steps.len() > before
        {
            let step_id = recording.steps.last().unwrap().id;
            for (offset, &id) in created.iter().enumerate() {
                recording
                    .targets
                    .insert(id, Target::Created { step_id, offset });
            }
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
                    self.macro_recording = Some(Recording::new(self));
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
                    self.macro_editor = Some(Editor {
                        target_keys: HashMap::new(),
                        next_id: self
                            .macro_recording
                            .as_ref()
                            .unwrap()
                            .steps
                            .iter()
                            .map(|c| c.id)
                            .max()
                            .unwrap_or(0)
                            .saturating_add(1),
                        definition: Definition {
                            version: 2,
                            name: self.text("レイヤー手順", "Layer steps").into(),
                            steps: self.macro_recording.as_ref().unwrap().steps.clone(),
                        },
                        path: None,
                        recording: true,
                        error: String::new(),
                    });
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
                self.open_macro_run(definition);
            }
            let mut edit = None;
            ui.menu_button(
                self.text("マクロの手順を編集…", "Edit macro steps…"),
                |ui| {
                    for (index, saved) in self.saved_macros.iter().enumerate() {
                        if ui.button(&saved.definition.name).clicked() {
                            edit = Some(index);
                            ui.close_menu();
                        }
                    }
                },
            );
            if let Some(index) = edit {
                let saved = &self.saved_macros[index];
                self.macro_editor = Some(Editor {
                    target_keys: HashMap::new(),
                    next_id: saved
                        .definition
                        .steps
                        .iter()
                        .map(|c| c.id)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1),
                    definition: saved.definition.clone(),
                    path: Some(saved.path.clone()),
                    recording: false,
                    error: String::new(),
                });
            }
        }
        ui.label(self.text(
            "記録対象: レイヤー作成・複製・名前・表示・不透明度・合成モード",
            "Records: layer creation, duplication, name, visibility, opacity and blend mode",
        ));
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

    #[cfg(test)]
    fn run_macro(&mut self, definition: &Definition) -> Result<(), String> {
        let definition = normalize(definition.clone());
        let start = self
            .doc
            .layers
            .get(self.selected_layer)
            .ok_or("開始レイヤーがありません")?
            .id;
        let bindings = suggested_bindings(&definition, &self.doc);
        self.run_macro_bound(&definition, start, &bindings)
    }

    fn run_macro_bound(
        &mut self,
        definition: &Definition,
        start: u64,
        bindings: &HashMap<u64, u64>,
    ) -> Result<(), String> {
        validate(definition)?;
        validate_bindings(definition, &self.doc, start, bindings)?;
        if self.macro_recording.is_some() {
            return Err("記録中のマクロは実行できません".into());
        }
        if self.history.is_active() || !self.active.is_empty() || self.filter_pending {
            return Err("編集中の操作を確定してから実行してください".into());
        }
        self.commit_pending_guide_edit();
        let selected_before = self.selected_layer;
        let editing_mask_before = self.editing_mask;
        self.history.begin();
        self.macro_replaying = true;
        self.selected_layer = self
            .doc
            .layers
            .iter()
            .position(|layer| layer.id == start)
            .unwrap();
        let mut result = Ok(());
        let mut created = HashMap::<u32, Vec<u64>>::new();
        for (index, command) in definition.steps.iter().enumerate() {
            let target_id = match &command.target {
                Target::Current => self.doc.layers.get(self.selected_layer).map(|l| l.id),
                Target::Start => Some(start),
                Target::External { key, .. } => bindings.get(key).copied(),
                Target::Created { step_id, offset } => created
                    .get(step_id)
                    .and_then(|ids| ids.get(*offset))
                    .copied(),
            };
            if let Some(target_index) =
                target_id.and_then(|id| self.doc.layers.iter().position(|l| l.id == id))
            {
                self.selected_layer = target_index;
            } else {
                result = Err(format!("{}手目: 対象レイヤーがありません", index + 1));
                break;
            }
            let before_ids: HashSet<_> = self.doc.layers.iter().map(|l| l.id).collect();
            if let Err(error) = self.apply_macro_step(&command.step) {
                result = Err(format!("{}手目: {error}", index + 1));
                break;
            }
            if matches!(command.step, Step::NewRaster | Step::DuplicateActive) {
                created.insert(
                    command.id,
                    self.doc
                        .layers
                        .iter()
                        .filter(|l| !before_ids.contains(&l.id))
                        .map(|l| l.id)
                        .collect(),
                );
                // Normal UI frames tidy the folder stack before the next
                // operation. Replay must do so too, after capturing the same
                // creation offsets as recording, within this transaction.
                let selected_id = self.doc.layers[self.selected_layer].id;
                for (to, id) in efude_canvas::layer_tree_order(&self.doc.layers)
                    .into_iter()
                    .enumerate()
                {
                    let from = self
                        .doc
                        .layers
                        .iter()
                        .position(|layer| layer.id == id)
                        .unwrap();
                    self.history.move_layer(&mut self.doc.layers, from, to);
                }
                self.selected_layer = self
                    .doc
                    .layers
                    .iter()
                    .position(|layer| layer.id == selected_id)
                    .unwrap();
            }
        }
        self.macro_replaying = false;
        if result.is_ok() {
            self.history.commit();
        } else {
            self.history.cancel(&mut self.doc);
            self.selected_layer = selected_before;
            self.editing_mask = editing_mask_before;
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
                if self.doc.layers.len() >= 2000 {
                    return Err("レイヤー数またはIDの上限です".into());
                }
                self.add_raster_layer();
            }
            Step::DuplicateActive => {
                let id = self.doc.layers[self.selected_layer].id;
                let copied = efude_canvas::subtree_ids(&self.doc.layers, id).len();
                if self.doc.layers.len() + copied > 2000 {
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

    fn legacy(step: Step) -> Command {
        Command {
            id: 0,
            target: Target::Current,
            step,
        }
    }
    fn command(id: u32, target: Target, step: Step) -> Command {
        Command { id, target, step }
    }

    #[test]
    fn macro_creates_layers_after_loading_the_maximum_id() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].id = u64::MAX;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("macro-ids.efude");
        efude_io::save(&path, &doc).unwrap();
        let mut app = EfudeApp::default();
        app.doc = efude_io::load(&path).unwrap();
        app.selected_layer = 0;
        let definition = Definition {
            version: 1,
            name: "Create layers".into(),
            steps: vec![legacy(Step::NewRaster), legacy(Step::DuplicateActive)],
        };
        app.run_macro(&definition).unwrap();
        assert_eq!(
            app.doc
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            vec![u64::MAX, 1, 2]
        );
        app.undo();
        assert_eq!(app.doc.layers.len(), 1);
        assert_eq!(app.doc.layers[0].id, u64::MAX);
        app.redo();
        assert_eq!(app.doc.layers.len(), 3);
    }

    #[test]
    fn macro_replays_on_another_document_as_one_undo_step() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(32, 32);
        let definition = Definition {
            version: 1,
            name: "Setup".into(),
            steps: vec![
                legacy(Step::NewRaster),
                legacy(Step::SetName { name: "Ink".into() }),
                legacy(Step::SetOpacity { value: 0.7 }),
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
        app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(8, 8));
        app.editing_mask = true;
        let state = app.history.state_token();
        let definition = Definition {
            version: 1,
            name: "Too many".into(),
            steps: vec![legacy(Step::NewRaster), legacy(Step::NewRaster)],
        };
        assert!(app.run_macro(&definition).is_err());
        assert_eq!(app.doc.layers.len(), 1999);
        assert_eq!(app.history.state_token(), state);
        assert!(!app.history.can_undo());
        assert!(app.editing_mask, "failed macro changed the painting target");
    }

    #[test]
    fn normal_layer_buttons_are_recorded_as_semantic_steps() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.macro_recording = Some(Recording::new(&app));
        app.add_raster_layer();
        app.duplicate_layer_subtree();
        assert!(matches!(
            app.macro_recording.as_ref().unwrap().steps.as_slice(),
            [
                Command {
                    target: Target::Start,
                    step: Step::NewRaster,
                    ..
                },
                Command {
                    target: Target::Created {
                        step_id: 1,
                        offset: 0
                    },
                    step: Step::DuplicateActive,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn repeated_folder_duplicates_replay_the_order_seen_while_recording() {
        let mut h = crate::tool_tests::Harness::new(8, 8);
        for id in [2, 3] {
            let mut child = efude_canvas::Layer::new(id, format!("Child{id}"), 8, 8);
            child.parent_id = Some(4);
            h.app.doc.layers.push(child);
        }
        let mut folder = efude_canvas::Layer::new(4, "Folder", 8, 8);
        folder.kind = LayerKind::Folder;
        h.app.doc.layers.push(folder);
        h.app.selected_layer = 3;
        h.frames(1);
        let original = h.app.doc.clone();
        h.app.macro_recording = Some(Recording::new(&h.app));
        for _ in 0..2 {
            h.app.duplicate_layer_subtree();
            // Normal UI frames normalize the folder stack before the next click.
            h.frames(1);
        }
        let recording = h.app.macro_recording.take().unwrap();
        let names = |app: &EfudeApp| {
            app.doc
                .layers
                .iter()
                .map(|layer| (layer.id, layer.parent_id, layer.name.clone()))
                .collect::<Vec<_>>()
        };
        let expected = names(&h.app);
        h.app.doc = original;
        h.app.history = History::default();
        h.app.selected_layer = 3;
        let before = names(&h.app);
        let mut definition = Definition {
            version: 2,
            name: "Duplicate folder twice".into(),
            steps: recording.steps,
        };
        h.app.run_macro(&definition).unwrap();
        h.frames(1);
        assert_eq!(names(&h.app), expected);
        h.app.undo();
        h.frames(1);
        assert_eq!(names(&h.app), before);
        assert!(!h.app.history.can_undo());
        let token = h.app.history.state_token();
        definition.steps.push(command(
            3,
            Target::Created {
                step_id: 2,
                offset: 1999,
            },
            Step::SetOpacity { value: 0.5 },
        ));
        assert!(h.app.run_macro(&definition).is_err());
        h.frames(1);
        assert_eq!(names(&h.app), before);
        assert_eq!(h.app.history.state_token(), token);
        assert!(h.app.history.can_redo());
        h.app.redo();
        h.frames(1);
        assert_eq!(names(&h.app), expected);
    }

    #[test]
    fn legacy_files_load_and_v2_round_trip_preserves_stable_references() {
        let old: Definition = serde_json::from_str(r#"{"version":1,"name":"Old","steps":[{"op":"new_raster"},{"op":"set_name","name":"Ink"}]}"#).unwrap();
        let mut definition = normalize(old);
        validate(&definition).unwrap();
        definition.steps[1].target = Target::Created {
            step_id: 1,
            offset: 0,
        };
        let round_trip: Definition =
            serde_json::from_slice(&serde_json::to_vec(&definition).unwrap()).unwrap();
        validate(&round_trip).unwrap();
        assert_eq!(round_trip.steps[1].target, definition.steps[1].target);
        definition.steps.swap(0, 1);
        assert!(validate(&definition).is_err());
        definition.steps.remove(1);
        assert!(validate(&definition).is_err());
    }

    #[test]
    fn bindings_are_frozen_before_renames_and_ambiguous_targets_require_mapping() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.doc.layers[0].name = "Ink".into();
        app.doc
            .layers
            .push(efude_canvas::Layer::new(2, "Ink", 8, 8));
        let target = Target::External {
            key: 99,
            name: "Ink".into(),
        };
        let definition = Definition {
            version: 2,
            name: "Targets".into(),
            steps: vec![
                command(
                    1,
                    target.clone(),
                    Step::SetName {
                        name: "Renamed".into(),
                    },
                ),
                command(2, target, Step::SetOpacity { value: 0.25 }),
            ],
        };
        let token = app.history.state_token();
        assert!(app.run_macro(&definition).is_err());
        assert_eq!(app.history.state_token(), token);
        assert!(!app.history.can_undo());
        let start = app.doc.layers[0].id;
        app.run_macro_bound(&definition, start, &HashMap::from([(99, 2)]))
            .unwrap();
        assert_eq!(app.doc.layers[1].name, "Renamed");
        assert_eq!(app.doc.layers[1].opacity, 0.25);
        assert_eq!(app.doc.layers[0].opacity, 1.0);
        app.history.undo_document(&mut app.doc);
        assert_eq!(app.doc.layers[1].name, "Ink");
        assert_eq!(app.doc.layers[1].opacity, 1.0);
    }

    #[test]
    fn recording_properties_uses_the_operated_row_and_coalesces_only_that_target() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.doc
            .layers
            .push(efude_canvas::Layer::new(2, "Other", 8, 8));
        app.macro_recording = Some(Recording::new(&app));
        let start = app.doc.layers[0].id;
        app.record_macro_step(Step::SetOpacity { value: 0.8 }, start);
        app.record_macro_step(Step::SetOpacity { value: 0.3 }, 2);
        app.record_macro_step(Step::SetOpacity { value: 0.4 }, 2);
        let recording = app.macro_recording.take().unwrap();
        assert_eq!(recording.steps.len(), 2);
        assert_eq!(recording.steps[0].target, Target::Start);
        assert!(matches!(
            recording.steps[1].target,
            Target::External { key: 2, .. }
        ));
        let definition = Definition {
            version: 2,
            name: "Rows".into(),
            steps: recording.steps,
        };
        app.run_macro(&definition).unwrap();
        assert_eq!(app.doc.layers[0].opacity, 0.8);
        assert_eq!(app.doc.layers[1].opacity, 0.4);
    }

    #[test]
    fn recording_visibility_on_selected_and_unselected_rows_replays_the_operated_layer() {
        for target_index in 0..2 {
            let mut app = EfudeApp::default();
            app.doc = Document::new(8, 8);
            app.doc
                .layers
                .push(efude_canvas::Layer::new(2, "Other", 8, 8));
            app.macro_recording = Some(Recording::new(&app));
            let target_id = app.doc.layers[target_index].id;
            let ctx = egui::Context::default();
            let frame = |app: &mut EfudeApp, events| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 800.0))),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.layers_ui(ui, ctx));
                    },
                );
            };
            frame(&mut app, Vec::new());
            let row = app
                .layer_rows
                .iter()
                .find(|(id, _)| *id == target_id)
                .unwrap()
                .1;
            let eye = Pos2::new(row.left() + 31.0, row.center().y);
            frame(&mut app, vec![egui::Event::PointerMoved(eye)]);
            for pressed in [true, false] {
                frame(
                    &mut app,
                    vec![egui::Event::PointerButton {
                        pos: eye,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            assert!(
                !app.doc.layers[target_index].visible,
                "the operated row was hidden"
            );
            assert_eq!(app.selected_layer, 0, "the eye does not change selection");
            let recording = app.macro_recording.take().unwrap();
            assert_eq!(recording.steps.len(), 1);
            let expected_target = if target_index == 0 {
                Target::Start
            } else {
                Target::External {
                    key: target_id,
                    name: "Other".into(),
                }
            };
            assert_eq!(recording.steps[0].target, expected_target);
            assert!(matches!(
                recording.steps[0].step,
                Step::SetVisibility { visible: false }
            ));
            app.undo();
            assert!(app.doc.layers.iter().all(|layer| layer.visible));
            assert!(!app.history.can_undo());
            app.redo();
            assert!(!app.doc.layers[target_index].visible);
            app.undo();
            app.run_macro(&Definition {
                version: 2,
                name: "Hide operated layer".into(),
                steps: recording.steps,
            })
            .unwrap();
            assert!(!app.doc.layers[target_index].visible);
            assert!(app.doc.layers[1 - target_index].visible);
            app.undo();
            assert!(app.doc.layers.iter().all(|layer| layer.visible));
            assert!(!app.history.can_undo());
            app.redo();
            assert!(!app.doc.layers[target_index].visible);
            assert!(app.doc.layers[1 - target_index].visible);
        }
    }

    #[test]
    fn editor_layer_keys_do_not_conflict_with_saved_keys_from_another_document() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].name = "Current ink".into();
        let mut editor = Editor {
            definition: Definition {
                version: 2,
                name: "Targets".into(),
                steps: vec![command(
                    1,
                    Target::External {
                        key: 1,
                        name: "Original ink".into(),
                    },
                    Step::SetOpacity { value: 0.5 },
                )],
            },
            path: None,
            recording: false,
            error: String::new(),
            next_id: 2,
            target_keys: HashMap::new(),
        };
        let choices = editor_targets(&mut editor, &doc, 100);
        assert!(matches!(choices[0].0, Target::External { key: 2, .. }));
        assert_eq!(editor_targets(&mut editor, &doc, 100)[0].0, choices[0].0);
        editor.definition.steps.push(command(
            2,
            choices[0].0.clone(),
            Step::SetVisibility { visible: false },
        ));
        validate(&editor.definition).unwrap();
    }

    #[test]
    fn folder_copy_child_targets_survive_rename_and_failure_preserves_redo() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        for id in [2, 3] {
            let mut child = efude_canvas::Layer::new(id, format!("Child{id}"), 8, 8);
            child.parent_id = Some(4);
            app.doc.layers.push(child);
        }
        let mut folder = efude_canvas::Layer::new(4, "Folder", 8, 8);
        folder.kind = LayerKind::Folder;
        app.doc.layers.push(folder);
        app.selected_layer = 3;
        let mut definition = Definition {
            version: 2,
            name: "Copy folder".into(),
            steps: vec![
                command(1, Target::Start, Step::DuplicateActive),
                command(
                    2,
                    Target::Created {
                        step_id: 1,
                        offset: 1,
                    },
                    Step::SetName {
                        name: "Copied child".into(),
                    },
                ),
            ],
        };
        app.run_macro_bound(&definition, 4, &HashMap::new())
            .unwrap();
        assert_eq!(app.doc.layers.len(), 7);
        assert!(
            app.doc
                .layers
                .iter()
                .any(|l| l.name == "Copied child" && l.parent_id != Some(4))
        );
        assert_eq!(
            app.doc.layers.iter().find(|l| l.id == 3).unwrap().name,
            "Child3"
        );
        app.history.undo_document(&mut app.doc);
        assert_eq!(app.doc.layers.len(), 4);
        assert!(app.history.can_redo());
        let token = app.history.state_token();
        app.selected_layer = 3;
        definition.steps.push(command(
            3,
            Target::Created {
                step_id: 1,
                offset: 1999,
            },
            Step::SetOpacity { value: 0.1 },
        ));
        assert!(
            app.run_macro_bound(&definition, 4, &HashMap::new())
                .is_err()
        );
        assert_eq!(app.doc.layers.len(), 4);
        assert_eq!(app.history.state_token(), token);
        assert_eq!(app.selected_layer, 3);
        assert!(app.history.can_redo());
        app.history.redo_document(&mut app.doc);
        assert_eq!(app.doc.layers.len(), 7);
        assert!(app.doc.layers.iter().any(|l| l.name == "Copied child"));
    }
}
