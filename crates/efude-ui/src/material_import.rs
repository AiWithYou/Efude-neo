// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Stage complete brush sets before applying a merge or replacement.
use super::*;
use std::collections::HashSet;
use std::{
    hash::{DefaultHasher, Hasher},
    io::Write,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Append,
    ReplaceAll,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Collision {
    Keep,
    #[default]
    Rename,
    Replace,
}

pub(crate) struct Preview {
    brushes: Vec<Brush>,
    name: String,
    mode: Mode,
    collision: Collision,
    selected: Vec<bool>,
    snapshot: Vec<Brush>,
    plan: Option<Result<Plan, String>>,
    error: String,
}

struct Plan {
    brushes: Vec<Brush>,
    added: usize,
    identical: usize,
    kept: usize,
    replaced: usize,
    renamed: Vec<(String, String)>,
}

struct HashWriter(DefaultHasher);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn signature(brush: &Brush, ignore_name: bool) -> Result<u64, String> {
    // Streaming avoids expanding texture bytes into millions of JSON Value nodes.
    let mut copy;
    let brush = if ignore_name {
        copy = brush.clone();
        copy.name.clear();
        &copy
    } else {
        brush
    };
    let mut out = HashWriter(DefaultHasher::new());
    serde_json::to_writer(&mut out, brush).map_err(|e| e.to_string())?;
    Ok(out.0.finish())
}

fn same_content(a: &Brush, b: &Brush) -> bool {
    if a.name == b.name {
        a == b
    } else {
        let mut copy = a.clone();
        copy.name.clone_from(&b.name);
        copy == *b
    }
}

fn merge(
    existing: &[Brush],
    incoming: &[Brush],
    selected: &[bool],
    mode: Mode,
    collision: Collision,
) -> Result<Plan, String> {
    let mut result = Plan {
        brushes: if mode == Mode::Append {
            existing.to_vec()
        } else {
            Vec::new()
        },
        added: 0,
        identical: 0,
        kept: 0,
        replaced: 0,
        renamed: Vec::new(),
    };
    let mut fingerprints = result
        .brushes
        .iter()
        .map(|brush| signature(brush, true))
        .collect::<Result<Vec<_>, _>>()?;
    let mut names: HashSet<String> = result.brushes.iter().map(|b| b.name.clone()).collect();
    for (index, brush) in incoming.iter().enumerate() {
        if !selected.get(index).copied().unwrap_or(false) {
            continue;
        }
        let fingerprint = signature(brush, true)?;
        if fingerprints
            .iter()
            .enumerate()
            .any(|(i, old)| *old == fingerprint && same_content(&result.brushes[i], brush))
        {
            result.identical += 1;
            continue;
        }
        let mut brush = brush.clone();
        if names.contains(&brush.name) {
            match collision {
                Collision::Keep => {
                    result.kept += 1;
                    continue;
                }
                Collision::Replace => {
                    // Replace every same-name preset; do not silently choose one of several.
                    for (index, old) in result.brushes.iter_mut().enumerate() {
                        if old.name == brush.name {
                            *old = brush.clone();
                            fingerprints[index] = fingerprint;
                            result.replaced += 1;
                        }
                    }
                    continue;
                }
                Collision::Rename => {
                    let original = brush.name.clone();
                    for suffix in 2..=100_000 {
                        let candidate = format!("{original} ({suffix})");
                        if names.insert(candidate.clone()) {
                            brush.name = candidate;
                            break;
                        }
                    }
                    if brush.name == original {
                        return Err("同じ名前のブラシが多すぎます".into());
                    }
                    result.renamed.push((original, brush.name.clone()));
                }
            }
        } else {
            names.insert(brush.name.clone());
        }
        result.brushes.push(brush);
        fingerprints.push(fingerprint);
        result.added += 1;
    }
    if result.brushes.is_empty() {
        return Err("少なくとも1本のブラシを残してください".into());
    }
    if result.brushes.len() > 512 {
        return Err("ブラシセットは512本までです。取り込むブラシを減らしてください".into());
    }
    Ok(result)
}

pub(crate) struct Undo {
    brushes: Vec<Brush>,
    applied: Vec<Brush>,
    selected: usize,
    tool_brushes: [Option<usize>; 4],
    size: f32,
    applied_selected: usize,
    applied_tool_brushes: [Option<usize>; 4],
    applied_size: f32,
}

impl EfudeApp {
    pub(crate) fn stage_brush_import(&mut self, brushes: Vec<Brush>, name: String) {
        let selected = vec![true; brushes.len()];
        self.material_import = Some(Preview {
            brushes,
            name,
            mode: Mode::Append,
            collision: Collision::Rename,
            selected,
            snapshot: self.brushes.clone(),
            plan: None,
            error: String::new(),
        });
    }

    pub(crate) fn undo_brush_import(&mut self) {
        let Some(undo) = self.material_import_undo.take() else {
            return;
        };
        // Later preset edits must not be lost by undoing an older import.
        if self.brushes != undo.applied {
            self.status = self
                .text(
                    "取り込み後にブラシを変更したため、取り込みだけは戻せません",
                    "Presets changed after import; the import cannot be undone alone",
                )
                .into();
            self.material_import_undo = Some(undo);
            return;
        }
        let remap = |index: usize| {
            undo.applied
                .get(index)
                .and_then(|brush| undo.brushes.iter().position(|old| old == brush))
        };
        let selected = if self.selected_brush == undo.applied_selected {
            undo.selected
        } else {
            remap(self.selected_brush).unwrap_or(undo.selected)
        };
        let tool_brushes = std::array::from_fn(|index| {
            if self.tool_brushes[index] == undo.applied_tool_brushes[index] {
                undo.tool_brushes[index]
            } else {
                self.tool_brushes[index].and_then(remap)
            }
        });
        let size = if self.size == undo.applied_size {
            undo.size
        } else {
            self.size
        };
        self.replace_brushes(undo.brushes);
        self.selected_brush = selected.min(self.brushes.len() - 1);
        self.tool_brushes = tool_brushes;
        self.size = size;
        self.status = self
            .text("ブラシの取り込みを戻しました", "Brush import undone")
            .into();
    }

    fn apply_brush_import(&mut self, plan: Plan) {
        if plan.brushes == self.brushes {
            return;
        }
        let mut undo = Undo {
            brushes: self.brushes.clone(),
            applied: Vec::new(),
            selected: self.selected_brush,
            tool_brushes: self.tool_brushes,
            size: self.size,
            applied_selected: 0,
            applied_tool_brushes: [None; 4],
            applied_size: 0.0,
        };
        // Importing additional presets must retain the brushes still present,
        // including each tool's choice and the current working size.
        let remap = |index: usize| {
            self.brushes.get(index).and_then(|brush| {
                if plan.brushes.get(index) == Some(brush) {
                    Some(index)
                } else {
                    plan.brushes.iter().position(|candidate| candidate == brush)
                }
            })
        };
        let selected = remap(self.selected_brush);
        let tool_brushes = self.tool_brushes.map(|index| index.and_then(remap));
        self.replace_brushes(plan.brushes);
        if let Some(selected) = selected {
            self.selected_brush = selected;
            self.size = undo.size;
        }
        self.tool_brushes = tool_brushes;
        undo.applied = self.brushes.clone();
        undo.applied_selected = self.selected_brush;
        undo.applied_tool_brushes = self.tool_brushes;
        undo.applied_size = self.size;
        self.material_import_undo = Some(undo);
    }

    pub(crate) fn brush_import_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut preview) = self.material_import.take() else {
            return;
        };
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut apply = None;
        if preview.plan.is_none() {
            preview.plan = Some(merge(
                &preview.snapshot,
                &preview.brushes,
                &preview.selected,
                preview.mode,
                preview.collision,
            ));
        }
        egui::Window::new(t("取り込むブラシを確認", "Review brush import"))
            .open(&mut open)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.label(&preview.name);
                let before = (preview.mode, preview.collision, preview.selected.clone());
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut preview.mode,
                        Mode::Append,
                        t("今のブラシに追加", "Append to current presets"),
                    );
                    ui.selectable_value(
                        &mut preview.mode,
                        Mode::ReplaceAll,
                        t("セット全体を置き換え", "Replace entire set"),
                    );
                });
                if preview.mode == Mode::ReplaceAll {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!(
                            "{} {}",
                            preview.snapshot.len(),
                            t(
                                "本の現在のブラシを外します",
                                "current brushes will be removed"
                            )
                        ),
                    );
                }
                ui.label(t(
                    "同じ内容は省略します。名前が同じで内容が違う場合：",
                    "Identical content is skipped. Same name, different content:",
                ));
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut preview.collision,
                        Collision::Rename,
                        t("別名で追加", "Add with another name"),
                    );
                    ui.selectable_value(
                        &mut preview.collision,
                        Collision::Keep,
                        t("今のものを残す", "Keep current"),
                    );
                    ui.selectable_value(
                        &mut preview.collision,
                        Collision::Replace,
                        t("同名を置き換え", "Replace same-name presets"),
                    );
                });
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (index, brush) in preview.brushes.iter().enumerate() {
                            ui.checkbox(&mut preview.selected[index], &brush.name);
                        }
                    });
                if before != (preview.mode, preview.collision, preview.selected.clone()) {
                    preview.plan = Some(merge(
                        &preview.snapshot,
                        &preview.brushes,
                        &preview.selected,
                        preview.mode,
                        preview.collision,
                    ));
                }
                if !preview.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &preview.error);
                    if ui
                        .button(t("現在のブラシで再確認", "Review current presets"))
                        .clicked()
                    {
                        preview.snapshot = self.brushes.clone();
                        preview.plan = None;
                        preview.error.clear();
                    }
                }
                if preview.plan.is_none() {
                    return;
                }
                match preview.plan.as_ref().unwrap() {
                    Ok(plan) => {
                        ui.label(format!(
                            "{}: {} · {}: {} · {}: {} · {}: {} · {}: {}",
                            t("追加", "Add"),
                            plan.added,
                            t("同一内容", "Identical"),
                            plan.identical,
                            t("現状維持", "Keep"),
                            plan.kept,
                            t("置換", "Replace"),
                            plan.replaced,
                            t("合計", "Total"),
                            plan.brushes.len()
                        ));
                        for (before, after) in &plan.renamed {
                            ui.label(format!("{before} → {after}"));
                        }
                        if ui
                            .add_enabled(
                                preview.error.is_empty() && (plan.brushes != preview.snapshot),
                                egui::Button::new(t("この内容で取り込む", "Apply import")),
                            )
                            .clicked()
                        {
                            if self.brushes == preview.snapshot {
                                apply = Some(preview.plan.take().unwrap().unwrap());
                            } else {
                                preview.error = t(
                                    "確認中にブラシが変わりました。現在の内容で再確認してください",
                                    "Presets changed during review. Review the current contents.",
                                )
                                .into();
                            }
                        }
                    }
                    Err(error) => {
                        ui.colored_label(Color32::LIGHT_RED, error);
                    }
                }
            });
        if let Some(plan) = apply {
            self.apply_brush_import(plan);
            self.status = t("ブラシを取り込みました", "Brushes imported").into();
            open = false;
        }
        if open {
            self.material_import = Some(preview);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_duplicates_name_collisions_and_set_replacement_are_distinct() {
        let original = crate::default_presets().remove(0);
        let mut same = original.clone();
        same.name = "別名".into();
        let mut changed = original.clone();
        changed.size += 1.0;
        let existing = vec![original.clone()];
        let incoming = vec![same, changed.clone()];
        let plan = merge(
            &existing,
            &incoming,
            &[true; 2],
            Mode::Append,
            Collision::Rename,
        )
        .unwrap();
        assert_eq!((plan.identical, plan.added), (1, 1));
        assert_ne!(plan.brushes[0].name, plan.brushes[1].name);
        assert_eq!(existing[0].size, original.size);
        let plan = merge(
            &existing,
            &incoming,
            &[true; 2],
            Mode::Append,
            Collision::Replace,
        )
        .unwrap();
        assert_eq!(plan.replaced, 1);
        assert_eq!(plan.brushes[0].size, changed.size);
        assert!(
            merge(
                &existing,
                &incoming,
                &[false; 2],
                Mode::ReplaceAll,
                Collision::Rename
            )
            .is_err()
        );
    }

    #[test]
    fn undo_import_keeps_later_size_and_noop_does_not_replace_undo() {
        let mut app = EfudeApp::default();
        let before = app.brushes.clone();
        let mut added = before[0].clone();
        added.size += 3.0;
        added.name = "Imported".into();
        let plan = merge(
            &before,
            &[added.clone()],
            &[true],
            Mode::Append,
            Collision::Rename,
        )
        .unwrap();
        app.apply_brush_import(plan);
        let count = app.brushes.len();
        let noop = merge(
            &app.brushes,
            &[added],
            &[true],
            Mode::Append,
            Collision::Rename,
        )
        .unwrap();
        app.apply_brush_import(noop);
        assert_eq!(app.brushes.len(), count);
        app.size = 123.0;
        app.undo_brush_import();
        assert_eq!(app.brushes, before);
        assert_eq!(app.size, 123.0);
    }

    #[test]
    fn adding_brushes_preserves_the_current_brush_size_and_tool_assignments() {
        let mut app = EfudeApp::default();
        let pen = app
            .brushes
            .iter()
            .enumerate()
            .filter(|(_, brush)| crate::brush_fits_tool(brush.kind, Tool::Brush))
            .nth(1)
            .map(|(index, _)| index)
            .unwrap();
        app.select_brush_preset(pen);
        app.size = 123.0;
        let assignments = app.tool_brushes;
        let mut added = app.brushes[0].clone();
        added.name = "Imported".into();
        added.size += 3.0;
        let plan = merge(
            &app.brushes,
            &[added],
            &[true],
            Mode::Append,
            Collision::Rename,
        )
        .unwrap();
        app.apply_brush_import(plan);
        assert_eq!(app.selected_brush, pen);
        assert_eq!(app.size, 123.0);
        assert_eq!(app.tool_brushes, assignments);
        app.undo_brush_import();
        assert_eq!(app.selected_brush, pen);
        assert_eq!(app.size, 123.0);
        assert_eq!(app.tool_brushes, assignments);
    }
}
