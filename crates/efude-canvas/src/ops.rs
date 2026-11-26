// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;
use std::collections::HashSet;

/// Crop to document coordinates [x, y, width, height] as one undoable edit.
#[allow(clippy::result_unit_err)]
pub fn crop_document(
    doc: &mut Document,
    [x, y, w, h]: [u32; 4],
    history: &mut History,
) -> Result<bool, ()> {
    if !valid_document_dimensions(w, h)
        || x.checked_add(w).is_none_or(|v| v > doc.width)
        || y.checked_add(h).is_none_or(|v| v > doc.height)
    {
        return Err(());
    }
    if (w, h) == (doc.width, doc.height) {
        return Ok(false);
    }
    history.resize_at(doc, w, h, doc.dpi, -(x as i32), -(y as i32));
    Ok(true)
}

/// Straight pixel alpha multiplied by the layer mask (absent tiles are white).
pub fn layer_alpha_selection(layer: &Layer, width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0; (width as usize) * (height as usize)];
    for ((tx, ty), tile) in layer.pixels.tiles() {
        let ox = tx * TILE_SIZE;
        let oy = ty * TILE_SIZE;
        for y in 0..TILE_SIZE.min(height.saturating_sub(oy)) {
            for x in 0..TILE_SIZE.min(width.saturating_sub(ox)) {
                let a = tile[((y * TILE_SIZE + x) * 4 + 3) as usize];
                let mask = layer.mask.as_ref().map_or(255, |m| {
                    m.pixel_or_tile_default(ox + x, oy + y, [255; 4])[0]
                });
                out[((oy + y) * width + ox + x) as usize] =
                    ((u16::from(a) * u16::from(mask) + 127) / 255) as u8;
            }
        }
    }
    out
}

// Visit allocated paint tiles only. Halftone phase uses document coordinates.
fn composite_sparse(
    doc: &Document,
    include: impl Fn(u64) -> bool,
    mut visit: impl FnMut(u32, u32, [u8; 4]),
) {
    let keys: HashSet<_> = doc
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Raster && l.visible && include(l.id))
        .flat_map(|l| l.pixels.tiles.keys().copied())
        .collect();
    for (tx, ty) in keys {
        let Some((tile, origin)) = tile_document(doc, tx, ty) else {
            continue;
        };
        for (i, p) in composite_transparent_at(&tile, origin, &include)
            .chunks_exact(4)
            .enumerate()
        {
            if p[3] > 0 {
                visit(
                    origin.0 + i as u32 % tile.width,
                    origin.1 + i as u32 / tile.width,
                    [p[0], p[1], p[2], p[3]],
                );
            }
        }
    }
}

/// Visible composite bounds (including vector paint), in document coordinates [x, y, width, height].
pub fn document_content_bounds(doc: &Document) -> Option<[u32; 4]> {
    let mut bounds = None;
    composite_sparse(
        doc,
        |_| true,
        |x, y, _| {
            let [left, top, right, bottom] = bounds.get_or_insert([x, y, x, y]);
            *left = (*left).min(x);
            *top = (*top).min(y);
            *right = (*right).max(x);
            *bottom = (*bottom).max(y);
        },
    );
    bounds.map(|[l, t, r, b]| [l, t, r - l + 1, b - t + 1])
}

fn editable(doc: &Document, layer: &Layer, require_visible: bool) -> bool {
    let mut current = Some(layer);
    let mut seen = HashSet::new();
    while let Some(l) = current {
        if l.locked || l.reference || (require_visible && !l.visible) || !seen.insert(l.id) {
            return false;
        }
        current = match l.parent_id {
            None => None,
            Some(id) => match doc
                .layers
                .iter()
                .find(|p| p.id == id && p.kind == LayerKind::Folder)
            {
                Some(p) => Some(p),
                None => return false,
            },
        };
    }
    true
}

/// Layer IDs that one merge would replace, in paint order.
pub fn merge_targets(doc: &Document, index: usize, folder: bool) -> Option<Vec<u64>> {
    let source = doc.layers.get(index)?;
    if folder {
        return (source.kind == LayerKind::Folder).then(|| subtree_ids(&doc.layers, source.id));
    }
    let lower = doc.layers[..index]
        .iter()
        .rev()
        .find(|l| l.parent_id == source.parent_id)?;
    Some(vec![lower.id, source.id])
}

pub fn merge_available(doc: &Document, index: usize, folder: bool) -> bool {
    compatible_targets(doc, index, folder, false).is_some()
}

// Clipped paint can borrow external base alpha. Baking it into an ordinary
// layer can also increase the base alpha seen by later clips.
fn clipped_merge_compatibility(
    doc: &Document,
    ids: &[u64],
    visible_folder: Option<u64>,
    check_upper: bool,
) -> MergeCompatibility {
    use MergeCompatibility::*;
    let targets: HashSet<_> = ids.iter().copied().collect();
    let painted = |layer: &Layer| layer.visible && layer.pixels.has_allocated_tiles();
    let mut clipped = doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| targets.contains(&l.id) && l.clipping && painted(l))
        .peekable();
    if clipped.peek().is_none() {
        return Exact;
    }
    let first = doc
        .layers
        .iter()
        .position(|l| targets.contains(&l.id))
        .unwrap();
    let last = doc
        .layers
        .iter()
        .rposition(|l| targets.contains(&l.id))
        .unwrap();
    let samplers = |layers: &[Layer]| {
        layers
            .iter()
            .filter(|l| painted(l))
            .filter_map(|l| LayerSampler::new(doc, l.id))
            .collect::<Vec<_>>()
    };
    let external: Vec<_> = samplers(&doc.layers[..first])
        .into_iter()
        .filter(|s| !doc.layers[s.layer_index].clipping)
        .collect();
    let above = if check_upper
        && doc.layers[last + 1..]
            .iter()
            .any(|l| l.clipping && painted(l))
    {
        samplers(&doc.layers[last + 1..])
    } else {
        Vec::new()
    };
    if external.is_empty() && above.is_empty() {
        return Exact;
    }
    let internal: Vec<_> = samplers(&doc.layers[first..=last])
        .into_iter()
        .filter(|s| {
            let l = &doc.layers[s.layer_index];
            targets.contains(&l.id) && !l.clipping
        })
        .collect();
    let mut result = Exact;
    for (index, layer) in clipped {
        let Some(clip) = LayerSampler::new(doc, layer.id) else {
            continue;
        };
        for ((tx, ty), tile) in layer.pixels.tiles() {
            let in_tile = |s: &LayerSampler| {
                doc.layers[s.layer_index]
                    .pixels
                    .tiles
                    .contains_key(&(tx, ty))
            };
            if !external.iter().any(in_tile)
                && !above
                    .iter()
                    .any(|s| doc.layers[s.layer_index].clipping && in_tile(s))
            {
                continue;
            }
            for (i, _) in tile.chunks_exact(4).enumerate().filter(|(_, p)| p[3] != 0) {
                let x = tx * TILE_SIZE + i as u32 % TILE_SIZE;
                let y = ty * TILE_SIZE + i as u32 / TILE_SIZE;
                let alpha = |s: &LayerSampler| {
                    let (p, opacity) = s.pixel_and_opacity(doc, x, y, visible_folder);
                    f32::from(p[3]) / 255. * opacity
                };
                if alpha(&clip) == 0. {
                    continue;
                }
                let accumulate = |base: f32, s: &LayerSampler| {
                    let a = alpha(s);
                    a + base * (1. - a)
                };
                let base = internal
                    .iter()
                    .take_while(|s| s.layer_index < index)
                    .fold(0., accumulate);
                if base >= 1. {
                    continue;
                }
                if external.iter().any(|s| alpha(s) > 0.) {
                    return Unavailable;
                }
                if base == 0. || above.is_empty() || result != Exact {
                    continue;
                }
                let mut coverage = internal
                    .iter()
                    .skip_while(|s| s.layer_index < index)
                    .fold(base, accumulate);
                for s in &above {
                    if coverage >= 1. {
                        break;
                    }
                    if doc.layers[s.layer_index].clipping {
                        if alpha(s) > 0. {
                            result = ChangesAppearance(MergeAppearanceReason::TranslucentClipBase);
                            break;
                        }
                    } else {
                        coverage = accumulate(coverage, s);
                    }
                }
            }
        }
    }
    result
}

fn compatible_targets(
    doc: &Document,
    index: usize,
    folder: bool,
    check_upper: bool,
) -> Option<(Vec<u64>, MergeCompatibility)> {
    let ids = merge_targets(doc, index, folder)?;
    let source = &doc.layers[index];
    if folder {
        let mut internal_base = false;
        for l in doc.layers.iter().filter(|l| ids.contains(&l.id)) {
            if !editable(doc, l, false) {
                return None;
            }
            if l.kind != LayerKind::Raster {
                continue;
            }
            let mut ancestor = Some(l);
            let mut visible = true;
            while let Some(node) = ancestor {
                if node.id == source.id {
                    break;
                }
                visible &= node.visible;
                ancestor = node
                    .parent_id
                    .and_then(|id| doc.layers.iter().find(|p| p.id == id));
            }
            if visible {
                if l.clipping && !internal_base {
                    return None;
                }
                internal_base |= !l.clipping;
            }
        }
    } else {
        let lower = doc.layers.iter().find(|l| l.id == ids[0])?;
        if (lower.clipping && !source.clipping)
            || [source, lower]
                .iter()
                .any(|l| l.kind != LayerKind::Raster || l.is_vector() || !editable(doc, l, true))
        {
            return None;
        }
        if lower.clipping {
            return Some((ids, MergeCompatibility::Exact));
        }
    }
    let compatibility =
        clipped_merge_compatibility(doc, &ids, folder.then_some(source.id), check_upper);
    (compatibility != MergeCompatibility::Unavailable).then_some((ids, compatibility))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeCompatibility {
    Exact,
    ChangesAppearance(MergeAppearanceReason),
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeAppearanceReason {
    AncestorOpacityOrMask,
    BackgroundBlend,
    TranslucentClipBase,
}

pub fn merge_compatibility(doc: &Document, index: usize, folder: bool) -> MergeCompatibility {
    use MergeCompatibility::*;
    let Some((ids, clip_result)) = compatible_targets(doc, index, folder, true) else {
        return Unavailable;
    };
    let source = &doc.layers[index];
    let mut parent = source.parent_id;
    while let Some(l) = parent.and_then(|id| doc.layers.iter().find(|p| p.id == id)) {
        if l.opacity < 1. || l.mask.is_some() {
            return ChangesAppearance(MergeAppearanceReason::AncestorOpacityOrMask);
        }
        parent = l.parent_id;
    }
    if doc
        .layers
        .iter()
        .any(|l| ids.contains(&l.id) && (l.linear_blend || l.blend != BlendMode::Normal))
    {
        return ChangesAppearance(MergeAppearanceReason::BackgroundBlend);
    }
    if !folder
        && doc
            .layers
            .iter()
            .find(|l| l.id == ids[0])
            .is_some_and(|l| l.clipping)
    {
        let base: HashSet<_> = doc.layers[..index]
            .iter()
            .filter(|l| !l.clipping)
            .map(|l| l.id)
            .collect();
        let mut translucent = false;
        composite_sparse(
            doc,
            |id| base.contains(&id),
            |_, _, p| translucent |= p[3] < 255,
        );
        if translucent {
            return ChangesAppearance(MergeAppearanceReason::TranslucentClipBase);
        }
    }
    clip_result
}

pub fn merge_down(doc: &mut Document, index: usize, history: &mut History) -> Option<usize> {
    let (ids, _) = compatible_targets(doc, index, false, false)?;
    let lower = doc.layers.iter().position(|l| l.id == ids[0])?;
    Some(merge_layers(
        doc,
        lower,
        &ids.into_iter().collect(),
        history,
    ))
}
pub fn merge_folder(doc: &mut Document, index: usize, history: &mut History) -> Option<usize> {
    let (ids, _) = compatible_targets(doc, index, true, false)?;
    Some(merge_layers(
        doc,
        index,
        &ids.into_iter().collect(),
        history,
    ))
}

pub fn folder_alpha_selection(doc: &Document, index: usize) -> Vec<u8> {
    let ids = subtree_ids(&doc.layers, doc.layers[index].id);
    let mut mask = vec![0; doc.width as usize * doc.height as usize];
    composite_sparse(
        doc,
        |id| ids.contains(&id),
        |x, y, p| mask[(y * doc.width + x) as usize] = p[3],
    );
    mask
}

fn merge_layers(
    doc: &mut Document,
    index: usize,
    ids: &HashSet<u64>,
    history: &mut History,
) -> usize {
    let source = &doc.layers[index];
    let mut result = Layer::new(source.id, source.name.clone(), doc.width, doc.height);
    result.parent_id = source.parent_id;
    result.visible = source.visible;
    result.clipping = source.kind == LayerKind::Raster && source.clipping;
    let mut subset = doc.clone();
    subset.layers.retain(|l| ids.contains(&l.id));
    for layer in &mut subset.layers {
        if result.clipping {
            layer.clipping = false;
        }
        if layer.parent_id == source.parent_id {
            layer.parent_id = None;
        }
        // Preserve hidden folder content so showing the result works as before.
        if layer.id == source.id {
            layer.visible = true;
        }
    }
    composite_sparse(
        &subset,
        |_| true,
        |x, y, p| result.pixels.set_pixel(x, y, p),
    );
    let index_after = doc.layers[..index]
        .iter()
        .filter(|l| !ids.contains(&l.id))
        .count();
    history.push_entry(HistoryEntry::DocumentState {
        width: doc.width,
        height: doc.height,
        dpi: doc.dpi,
        layers: doc.layers.clone(),
        guide: doc.guide.clone(),
    });
    doc.layers.retain(|l| !ids.contains(&l.id));
    doc.layers.insert(index_after, result);
    index_after
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    Linear,
    Radial,
    Reflected,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub position: f32,
    pub color: [u8; 4],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeKind {
    Rectangle,
    RoundedRect { radius: f32 },
    Ellipse,
    Polygon { sides: u32 },
    Line,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeStyle {
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<([u8; 4], f32)>,
}

pub fn paint_over(base: [u8; 4], color: [u8; 4], coverage: f32, locked: bool) -> [u8; 4] {
    let a = coverage * (color[3] as f32 / 255.);
    if a <= 0. || (locked && base[3] == 0) {
        return base;
    }
    let retained = if locked {
        1. - a
    } else {
        base[3] as f32 / 255. * (1. - a)
    };
    let total = a + retained;
    let mut out = base;
    for c in 0..3 {
        out[c] = ((color[c] as f32 * a + base[c] as f32 * retained) / total).round() as u8;
    }
    if !locked {
        out[3] = (total * 255.).round() as u8;
    }
    out
}

// Record each changed storage tile once. Callers may group several draws.
fn draw_pixels(
    layer: &mut Layer,
    width: u32,
    height: u32,
    bounds: [u32; 4],
    selection: Option<&[u8]>,
    history: &mut History,
    paint: impl Fn(f32, f32, [u8; 4], f32, bool) -> [u8; 4],
) -> bool {
    if layer.locked
        || layer.reference
        || layer.is_vector()
        || layer.kind != LayerKind::Raster
        || (width, height) != (layer.pixels.width, layer.pixels.height)
    {
        return false;
    }
    let own = !history.is_active();
    if own {
        history.begin();
    }
    let [x0, y0, x1, y1] = bounds;
    let mut changed = false;
    for ty in y0 / TILE_SIZE..y1.div_ceil(TILE_SIZE) {
        for tx in x0 / TILE_SIZE..x1.div_ceil(TILE_SIZE) {
            let mut recorded = false;
            for y in y0.max(ty * TILE_SIZE)..y1.min((ty + 1) * TILE_SIZE) {
                for x in x0.max(tx * TILE_SIZE)..x1.min((tx + 1) * TILE_SIZE) {
                    let k = selection.map_or(255, |m| {
                        m.get((y * width + x) as usize).copied().unwrap_or(0)
                    });
                    if k == 0 {
                        continue;
                    }
                    let base = layer.pixels.pixel(x, y);
                    if layer.alpha_lock && base[3] == 0 {
                        continue;
                    }
                    let out = paint(
                        x as f32 + 0.5,
                        y as f32 + 0.5,
                        base,
                        k as f32 / 255.,
                        layer.alpha_lock,
                    );
                    if out != base {
                        if !recorded {
                            history.record_tile(layer, tx, ty);
                            recorded = true;
                        }
                        layer.pixels.set_pixel(x, y, out);
                        changed = true;
                    }
                }
            }
        }
    }
    if own {
        history.commit();
    }
    changed
}

/// Prepared shape or gradient evaluated at document pixel centres.
pub struct GeometryPaint {
    bounds: [u32; 4],
    sample: Box<dyn Fn(f32, f32, [u8; 4], f32, bool) -> [u8; 4]>,
}
impl GeometryPaint {
    pub fn pixel(&self, x: u32, y: u32, base: [u8; 4], coverage: f32, locked: bool) -> [u8; 4] {
        (self.sample)(x as f32 + 0.5, y as f32 + 0.5, base, coverage, locked)
    }
    pub fn draw(
        &self,
        layer: &mut Layer,
        size: [u32; 2],
        selection: Option<&[u8]>,
        history: &mut History,
    ) -> bool {
        draw_pixels(
            layer,
            size[0],
            size[1],
            self.bounds,
            selection,
            history,
            &self.sample,
        )
    }
}

#[allow(clippy::too_many_arguments)] // Public drawing API includes destination, selection and undo.
pub fn draw_gradient(
    layer: &mut Layer,
    width: u32,
    height: u32,
    start: [f32; 2],
    end: [f32; 2],
    kind: GradientKind,
    stops: &[GradientStop],
    selection: Option<&[u8]>,
    history: &mut History,
) -> bool {
    prepare_gradient([width, height], start, end, kind, stops)
        .is_some_and(|paint| paint.draw(layer, [width, height], selection, history))
}
pub fn prepare_gradient(
    [width, height]: [u32; 2],
    start: [f32; 2],
    end: [f32; 2],
    kind: GradientKind,
    stops: &[GradientStop],
) -> Option<GeometryPaint> {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let length = dx.hypot(dy);
    if !length.is_finite()
        || length == 0.
        || stops.is_empty()
        || stops.iter().any(|s| !s.position.is_finite())
    {
        return None;
    }
    let mut stops = stops.to_vec();
    stops.sort_by(|a, b| a.position.total_cmp(&b.position));
    Some(GeometryPaint {
        bounds: [0, 0, width, height],
        sample: Box::new(move |x, y, base, k, lock| {
            let (x, y) = (x - start[0], y - start[1]);
            let projection = (x * (dx / length) + y * (dy / length)) / length;
            let t = match kind {
                GradientKind::Radial => x.hypot(y) / length,
                GradientKind::Linear => projection,
                GradientKind::Reflected => projection.abs(),
            }
            .clamp(0., 1.);
            let next = stops.partition_point(|s| s.position <= t);
            let color = if next == 0 {
                stops[0].color
            } else if next == stops.len() {
                stops[next - 1].color
            } else {
                let (a, b) = (stops[next - 1], stops[next]);
                let f = (t - a.position) / (b.position - a.position);
                std::array::from_fn(|c| {
                    (a.color[c] as f32 + (b.color[c] as f32 - a.color[c] as f32) * f).round() as u8
                })
            };
            paint_over(base, color, k, lock)
        }),
    })
}

fn segment_distance(x: f32, y: f32, [ax, ay]: [f32; 2], [bx, by]: [f32; 2]) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy).max(f32::MIN_POSITIVE))
        .clamp(0., 1.);
    (x - ax - t * dx).hypot(y - ay - t * dy)
}

#[allow(clippy::too_many_arguments)] // Public drawing API includes destination, selection and undo.
pub fn draw_shape(
    layer: &mut Layer,
    width: u32,
    height: u32,
    start: [f32; 2],
    end: [f32; 2],
    kind: ShapeKind,
    style: ShapeStyle,
    selection: Option<&[u8]>,
    history: &mut History,
) -> bool {
    prepare_shape([width, height], start, end, kind, style)
        .is_some_and(|paint| paint.draw(layer, [width, height], selection, history))
}
pub fn prepare_shape(
    [width, height]: [u32; 2],
    start: [f32; 2],
    end: [f32; 2],
    kind: ShapeKind,
    style: ShapeStyle,
) -> Option<GeometryPaint> {
    if start.into_iter().chain(end).any(|v| !v.is_finite()) {
        return None;
    }
    let [cx, cy] = std::array::from_fn(|i| (start[i] + end[i]) * 0.5);
    let [rx, ry] = std::array::from_fn(|i| (end[i] - start[i]).abs() * 0.5);
    if kind != ShapeKind::Line && rx.min(ry) <= 0. {
        return None;
    }
    let stroke = style.stroke.filter(|(_, w)| w.is_finite() && *w > 0.);
    let pad = stroke.map_or(1., |(_, w)| w * 0.5 + 1.);
    let bounds = [
        (cx - rx - pad).floor() as u32,
        (cy - ry - pad).floor() as u32,
        ((cx + rx + pad).ceil() as u32).min(width),
        ((cy + ry + pad).ceil() as u32).min(height),
    ];
    let mut vertices = Vec::new();
    if let ShapeKind::Polygon { sides } = kind {
        let n = sides.clamp(3, 12);
        for i in 0..n {
            let angle = i as f32 * std::f32::consts::TAU / n as f32 - std::f32::consts::FRAC_PI_2;
            let radius = rx.min(ry);
            vertices.push([radius * angle.cos(), radius * angle.sin()]);
        }
    }
    Some(GeometryPaint {
        bounds,
        sample: Box::new(move |x, y, base, k, lock| {
            let (px, py) = (x - cx, y - cy);
            let distance = match kind {
                ShapeKind::Line => segment_distance(x, y, start, end),
                ShapeKind::Rectangle | ShapeKind::RoundedRect { .. } => {
                    let radius = match kind {
                        ShapeKind::RoundedRect { radius } => radius.clamp(0., rx.min(ry)),
                        _ => 0.,
                    };
                    let (qx, qy) = (px.abs() - rx + radius, py.abs() - ry + radius);
                    qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - radius
                }
                ShapeKind::Ellipse => {
                    let k0 = (px / rx).hypot(py / ry);
                    let k1 = (px / (rx * rx)).hypot(py / (ry * ry));
                    if k1 == 0. {
                        -rx.min(ry)
                    } else {
                        k0 * (k0 - 1.) / k1
                    }
                }
                ShapeKind::Polygon { .. } => {
                    let mut distance = f32::INFINITY;
                    let mut inside = true;
                    let mut a = *vertices.last().unwrap();
                    for &b in &vertices {
                        distance = distance.min(segment_distance(px, py, a, b));
                        inside &= (b[0] - a[0]) * (py - a[1]) >= (b[1] - a[1]) * (px - a[0]);
                        a = b;
                    }
                    if inside { -distance } else { distance }
                }
            };
            let mut ink = [0; 4];
            if kind != ShapeKind::Line
                && let Some(fill) = style.fill
            {
                ink = paint_over(ink, fill, (0.5 - distance).clamp(0., 1.), false);
            }
            if let Some((color, weight)) = stroke {
                ink = paint_over(
                    ink,
                    color,
                    (0.5 + weight * 0.5 - distance.abs()).clamp(0., 1.),
                    false,
                );
            }
            paint_over(base, ink, k, lock)
        }),
    })
}
