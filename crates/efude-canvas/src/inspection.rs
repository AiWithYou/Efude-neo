// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only finishing checks. Results are candidates, not automatic corrections.
use crate::{Document, LayerKind, TILE_SIZE};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hole,
    Speck,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: Kind,
    /// Inclusive minimum, exclusive maximum in document pixels.
    pub bounds: [u32; 4],
    pub area: u64,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub alpha_threshold: u8,
    pub hole_area: u32,
    pub speck_area: u32,
    pub finishing_bounds: Option<[u32; 4]>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            alpha_threshold: 8,
            hole_area: 64,
            speck_area: 16,
            finishing_bounds: None,
        }
    }
}

#[derive(Default, Debug)]
pub struct Report {
    pub candidates: Vec<Candidate>,
    pub counts: [usize; 3],
    pub hidden_layers: usize,
}

pub fn effectively_visible(doc: &Document, layer: &crate::Layer) -> bool {
    if !layer.visible || layer.opacity <= 0.0 {
        return false;
    }
    let mut parent = layer.parent_id;
    for _ in 0..doc.layers.len() {
        let Some(group) = parent.and_then(|id| doc.layers.iter().find(|group| group.id == id))
        else {
            break;
        };
        if !group.visible || group.opacity <= 0.0 {
            return false;
        }
        parent = group.parent_id;
    }
    true
}

fn filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>, String> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| "チェックに必要なメモリを確保できません")?;
    values.resize(count, value);
    Ok(values)
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("チェックを中止しました".into())
    } else {
        Ok(())
    }
}

/// The alpha of chosen layers, with original visibility, masks and clipping.
/// Excluded layers still supply clipping alpha; their artwork is not included.
fn alpha(doc: &Document, selected: &HashSet<u64>, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let count = doc.width as usize * doc.height as usize;
    let has_clipping = doc
        .layers
        .iter()
        .any(|l| l.clipping && selected.contains(&l.id));
    if count * if has_clipping { 6 } else { 1 } > 512 * 1024 * 1024 {
        return Err("チェック用画像が大きすぎます。クリッピング以外のレイヤーに対象を絞るか、小さい複製で確認してください".into());
    }
    let mut out = filled(count, 0u8)?;
    let mut composed = if has_clipping {
        filled(count, 0u8)?
    } else {
        Vec::new()
    };
    let mut base = if has_clipping {
        filled(count, 0f32)?
    } else {
        Vec::new()
    };
    let indices: HashMap<_, _> = doc
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();
    for layer in doc.layers.iter().filter(|l| l.kind == LayerKind::Raster) {
        cancelled(cancel)?;
        let mut visible = layer.visible;
        let mut opacity = layer.opacity.clamp(0.0, 1.0);
        let mut masks = Vec::new();
        let mut parent = layer.parent_id;
        for _ in 0..doc.layers.len() {
            let Some(group) = parent
                .and_then(|id| indices.get(&id))
                .map(|&i| &doc.layers[i])
            else {
                break;
            };
            visible &= group.visible;
            opacity *= group.opacity.clamp(0.0, 1.0);
            if let Some(mask) = &group.mask {
                masks.push(mask);
            }
            parent = group.parent_id;
        }
        if !visible {
            continue;
        }
        let include = selected.contains(&layer.id);
        if !include && !has_clipping {
            continue;
        }
        for ((tx, ty), tile) in layer.pixels.tiles() {
            cancelled(cancel)?;
            let (ox, oy) = (tx * TILE_SIZE, ty * TILE_SIZE);
            if ox >= doc.width || oy >= doc.height {
                continue;
            }
            let tile = crate::display_tile(layer, doc.dpi, tx, ty, (0, 0), tile);
            let ancestor_tiles: Vec<_> = masks.iter().filter_map(|m| m.tile_data(tx, ty)).collect();
            let own_mask = layer.mask.as_ref().and_then(|m| m.tile_data(tx, ty));
            for y in 0..TILE_SIZE.min(doc.height - oy) {
                for x in 0..TILE_SIZE.min(doc.width - ox) {
                    let i = ((y * TILE_SIZE + x) * 4) as usize;
                    let p = ((oy + y) * doc.width + ox + x) as usize;
                    let mut sa = tile[i + 3] as f32 / 255.0 * opacity;
                    for mask in &ancestor_tiles {
                        sa *= mask[i] as f32 / 255.0;
                    }
                    if let Some(mask) = own_mask {
                        sa *= mask[i] as f32 / 255.0;
                    }
                    if layer.clipping {
                        sa *= base[p];
                    }
                    if sa <= 0.0 {
                        continue;
                    }
                    if include {
                        let da = out[p] as f32 / 255.0;
                        out[p] = ((sa + da * (1.0 - sa)) * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    if has_clipping {
                        let oa = sa + composed[p] as f32 / 255.0 * (1.0 - sa);
                        composed[p] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
                        if !layer.clipping {
                            base[p] = oa;
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Inspect immutable snapshots on a worker. Zero area disables that category.
pub fn inspect(
    doc: &Document,
    layer_ids: &[u64],
    options: &Options,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    if !crate::valid_document_dimensions(doc.width, doc.height) || layer_ids.is_empty() {
        return Err("チェック対象のレイヤーまたは画像サイズが不正です".into());
    }
    let selected: HashSet<_> = layer_ids.iter().copied().collect();
    if !selected.iter().all(|id| {
        doc.layers
            .iter()
            .any(|l| l.id == *id && l.kind == LayerKind::Raster)
    }) {
        return Err("チェック対象のレイヤーがありません".into());
    }
    let hidden_layers = doc
        .layers
        .iter()
        .filter(|layer| selected.contains(&layer.id) && !effectively_visible(doc, layer))
        .count();
    if hidden_layers == selected.len() {
        return Err(
            "対象レイヤーがすべて非表示または不透明度0です。表示状態を確認してください".into(),
        );
    }
    let mut mask = alpha(doc, &selected, cancel)?;
    for chunk in mask.chunks_mut(4096) {
        cancelled(cancel)?;
        for value in chunk {
            *value = u8::from(*value > options.alpha_threshold);
        }
    }
    let mut report = inspect_mask(doc.width, doc.height, &mut mask, options, cancel)?;
    report.hidden_layers = hidden_layers;
    Ok(report)
}

fn inspect_mask(
    width: u32,
    height: u32,
    mask: &mut [u8],
    options: &Options,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    let mut report = Report::default();
    let finishing = options.finishing_bounds.map(|b| {
        [
            b[0].min(width),
            b[1].min(height),
            b[2].min(width),
            b[3].min(height),
        ]
    });
    if finishing.is_some_and(|b| b[0] >= b[2] || b[1] >= b[3]) {
        return Err("仕上がり範囲が空です".into());
    }
    // Mark whole horizontal runs when enqueuing. Large components must still
    // be completely visited, even after exceeding the candidate area limit.
    let mut stack = Vec::new();
    for y in 0..height {
        cancelled(cancel)?;
        for x in 0..width {
            let color = mask[(y * width + x) as usize];
            if color > 1 {
                continue;
            }
            let first = mark_run(mask, width, y, x, color);
            stack.push((y, first.0, first.1));
            let mut bounds = [width, height, 0, 0];
            let mut area = 0u64;
            let mut edge = false;
            let mut outside = 0u64;
            let mut outside_bounds = [width, height, 0, 0];
            let mut runs = 0usize;
            while let Some((ry, left, right)) = stack.pop() {
                if runs & 255 == 0 {
                    cancelled(cancel)?;
                }
                runs += 1;
                area += (right - left + 1) as u64;
                bounds[0] = bounds[0].min(left);
                bounds[1] = bounds[1].min(ry);
                bounds[2] = bounds[2].max(right + 1);
                bounds[3] = bounds[3].max(ry + 1);
                edge |= left == 0 || right + 1 == width || ry == 0 || ry + 1 == height;
                if color == 1
                    && let Some(b) = finishing
                {
                    for px in left..=right {
                        if px < b[0] || px >= b[2] || ry < b[1] || ry >= b[3] {
                            outside += 1;
                            outside_bounds[0] = outside_bounds[0].min(px);
                            outside_bounds[1] = outside_bounds[1].min(ry);
                            outside_bounds[2] = outside_bounds[2].max(px + 1);
                            outside_bounds[3] = outside_bounds[3].max(ry + 1);
                        }
                    }
                }
                for ny in [ry.checked_sub(1), (ry + 1 < height).then_some(ry + 1)]
                    .into_iter()
                    .flatten()
                {
                    // Eight-connected ink preserves diagonal strokes; four-connected
                    // transparency identifies holes enclosed by those strokes.
                    let mut nx = if color == 1 {
                        left.saturating_sub(1)
                    } else {
                        left
                    };
                    let end = if color == 1 {
                        (right + 1).min(width - 1)
                    } else {
                        right
                    };
                    while nx <= end {
                        if mask[(ny * width + nx) as usize] == color {
                            let run = mark_run(mask, width, ny, nx, color);
                            stack.push((ny, run.0, run.1));
                            if stack.len() > 1_000_000 {
                                return Err(
                                    "細かい領域が多すぎます。対象レイヤーを絞ってください".into()
                                );
                            }
                            nx = run.1 + 1;
                        } else {
                            nx += 1;
                        }
                    }
                }
            }
            if color == 0 && !edge && area <= options.hole_area as u64 {
                add(&mut report, Kind::Hole, bounds, area);
            }
            if color == 1 && area <= options.speck_area as u64 {
                add(&mut report, Kind::Speck, bounds, area);
            }
            if outside > 0 {
                add(&mut report, Kind::Outside, outside_bounds, outside);
            }
        }
    }
    Ok(report)
}

fn mark_run(mask: &mut [u8], width: u32, y: u32, x: u32, color: u8) -> (u32, u32) {
    let row = &mut mask[(y * width) as usize..((y + 1) * width) as usize];
    let (mut left, mut right) = (x, x);
    while left > 0 && row[(left - 1) as usize] == color {
        left -= 1;
    }
    while right + 1 < width && row[(right + 1) as usize] == color {
        right += 1;
    }
    row[left as usize..=right as usize].fill(color + 2);
    (left, right)
}

fn add(report: &mut Report, kind: Kind, bounds: [u32; 4], area: u64) {
    report.counts[match kind {
        Kind::Hole => 0,
        Kind::Speck => 1,
        Kind::Outside => 2,
    }] += 1;
    if report.candidates.len() < 500 {
        report.candidates.push(Candidate { kind, bounds, area });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosed_hole_speck_and_outside_are_candidates_without_changing_pixels() {
        let mut doc = Document::new(16, 16);
        for y in 2..10 {
            for x in 2..10 {
                doc.layers[0].pixels.set_pixel(x, y, [80, 40, 10, 255]);
            }
        }
        doc.layers[0].pixels.set_pixel(5, 5, [0; 4]);
        doc.layers[0].pixels.set_pixel(14, 14, [0, 0, 0, 255]);
        let before = doc.layers[0].pixels.to_dense();
        let options = Options {
            finishing_bounds: Some([1, 1, 12, 12]),
            ..Options::default()
        };
        let report = inspect(&doc, &[doc.layers[0].id], &options, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.counts, [1, 1, 1]);
        assert_eq!(report.candidates[0].bounds, [5, 5, 6, 6]);
        assert_eq!(before, doc.layers[0].pixels.to_dense());
    }

    #[test]
    fn diagonal_openings_and_strokes_are_connected_and_large_regions_not_fragmented() {
        let options = Options {
            hole_area: 2,
            speck_area: 2,
            ..Options::default()
        };
        let mut mask = vec![1; 25];
        for p in [0, 6, 12] {
            mask[p] = 0;
        }
        let report = inspect_mask(5, 5, &mut mask, &options, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.counts, [2, 0, 0]);
        let mut mask = vec![0; 25];
        for p in [0, 6, 12, 18, 24] {
            mask[p] = 1;
        }
        let report = inspect_mask(5, 5, &mut mask, &options, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.counts, [0, 0, 0]);
    }

    #[test]
    fn excluded_base_still_supplies_clipping_and_masks_are_respected() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set_pixel(2, 2, [0, 0, 0, 128]);
        let mut clip = crate::Layer::new(2, "clip", 8, 8);
        clip.clipping = true;
        clip.pixels.set_pixel(2, 2, [0, 0, 0, 255]);
        clip.pixels.set_pixel(3, 3, [0, 0, 0, 255]);
        doc.layers.push(clip);
        let values = alpha(&doc, &HashSet::from([2]), &AtomicBool::new(false)).unwrap();
        assert_eq!(values[18], 128);
        assert_eq!(values[27], 0);
        doc.layers[1].opacity = 0.5;
        assert_eq!(
            alpha(&doc, &HashSet::from([2]), &AtomicBool::new(false)).unwrap()[18],
            64
        );
        doc.layers[0].visible = false;
        assert_eq!(
            alpha(&doc, &HashSet::from([2]), &AtomicBool::new(false)).unwrap()[18],
            0
        );
    }

    #[test]
    fn inspection_alpha_matches_the_existing_renderer_with_paper_groups_and_masks() {
        let mut doc = Document::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                doc.layers[0].pixels.set_pixel(x, y, [255; 4]);
            }
        }
        let mut group = crate::Layer::new(2, "Group", 8, 8);
        group.kind = LayerKind::Folder;
        group.opacity = 0.6;
        let mut ink = crate::Layer::new(3, "Base", 8, 8);
        ink.parent_id = Some(2);
        ink.pixels.set_pixel(2, 2, [10, 20, 30, 128]);
        let mut clip = crate::Layer::new(4, "Clip", 8, 8);
        clip.parent_id = Some(2);
        clip.clipping = true;
        clip.pixels.set_pixel(2, 2, [0, 0, 0, 255]);
        clip.pixels.set_pixel(3, 3, [0, 0, 0, 255]);
        let mut mask = crate::TilePixels::new(8, 8);
        mask.ensure_tile_filled(0, 0, [255; 4]);
        mask.set_pixel(3, 3, [40; 4]);
        clip.mask = Some(mask);
        doc.layers.extend([group, ink, clip]);
        for paper_visible in [true, false] {
            for group_visible in [true, false] {
                doc.layers[0].visible = paper_visible;
                doc.layers[1].visible = group_visible;
                let chosen = doc
                    .layers
                    .iter()
                    .map(|layer| layer.id)
                    .collect::<HashSet<_>>();
                let values = alpha(&doc, &chosen, &AtomicBool::new(false)).unwrap();
                let rendered = crate::composite_transparent(&doc);
                assert!(
                    values
                        .iter()
                        .zip(rendered.chunks_exact(4))
                        .all(|(a, pixel)| *a == pixel[3])
                );
            }
        }
        doc.layers[1].visible = false;
        assert!(inspect(&doc, &[4], &Options::default(), &AtomicBool::new(false)).is_err());
    }
}
