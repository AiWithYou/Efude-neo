// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;
use glam::DVec2;

#[derive(Clone, Copy)]
struct Bounds {
    min: DVec2,
    max: DVec2,
}
impl Bounds {
    fn from_rect([x, y, w, h]: [u32; 4]) -> Option<Self> {
        let min = DVec2::new(x.into(), y.into());
        Some(Self {
            min,
            max: min + DVec2::new(w.checked_sub(1)?.into(), h.checked_sub(1)?.into()),
        })
    }
    fn point(p: DVec2) -> Self {
        Self { min: p, max: p }
    }
    fn add(&mut self, p: DVec2) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    fn contains(self, p: DVec2) -> bool {
        p.cmpge(self.min - 1e-7).all() && p.cmple(self.max + 1e-7).all()
    }
    fn pixels(self, size: DVec2) -> impl Iterator<Item = (u32, u32)> {
        let lo = self.min.ceil().max(DVec2::ZERO);
        let hi = self.max.floor().min(size - 1.);
        (lo.y as i64..=hi.y as i64)
            .flat_map(move |y| (lo.x as i64..=hi.x as i64).map(move |x| (x as u32, y as u32)))
    }
}

struct Cell {
    source: Bounds,
    dest: Bounds,
    origin: DVec2,
    u: DVec2,
    v: DVec2,
    uv: DVec2,
}
impl Cell {
    fn new(source: Bounds, [a, b, c, d]: [DVec2; 4]) -> Self {
        let mut dest = Bounds::point(a);
        for p in [b, c, d] {
            dest.add(p);
        }
        Self {
            source,
            dest,
            origin: a,
            u: b - a,
            v: c - a,
            uv: a - b - c + d,
        }
    }
    fn inverse(&self, p: DVec2) -> Option<DVec2> {
        if !self.dest.contains(p) {
            return None;
        }
        let delta = p - self.origin;
        // Cross delta - u*U with V + u*UV to eliminate v.
        let a = -self.u.perp_dot(self.uv);
        let b = delta.perp_dot(self.uv) - self.u.perp_dot(self.v);
        let c = delta.perp_dot(self.v);
        let mut roots = [f64::NAN; 2];
        if a.abs() <= 1e-13 * b.abs().max(1.) {
            if b.abs() < 1e-14 {
                return None;
            }
            roots[0] = -c / b;
        } else {
            let disc = b * b - 4. * a * c;
            if disc < 0. {
                return None;
            }
            let q = -0.5 * (b + disc.sqrt().copysign(b));
            roots = if q == 0. {
                [-b / (2. * a), f64::NAN]
            } else {
                [q / a, c / q]
            };
        }
        for u in roots {
            if !(-1e-8..=1. + 1e-8).contains(&u) {
                continue;
            }
            let direction = self.v + u * self.uv;
            let residual = delta - u * self.u;
            let v = residual.dot(direction) / direction.length_squared();
            if (-1e-8..=1. + 1e-8).contains(&v)
                && (residual - v * direction).length_squared() <= 1e-12
            {
                let uv = DVec2::new(u, v).clamp(DVec2::ZERO, DVec2::ONE);
                return Some(self.source.min + uv * (self.source.max - self.source.min));
            }
        }
        None
    }
}

/// Scale, rotation in radians and translation in document pixels.
#[derive(Clone, Copy)]
pub struct AffineTransform {
    scale: [f32; 2],
    angle: f32,
    offset: [f32; 2],
}
impl AffineTransform {
    pub fn components(self) -> ([f32; 2], f32, [f32; 2]) {
        (self.scale, self.angle, self.offset)
    }
    pub fn validated(scale: [f32; 2], angle: f32, offset: [f32; 2], layer: &Layer) -> Option<Self> {
        if scale
            .into_iter()
            .chain(offset)
            .chain([angle])
            .any(|v| !v.is_finite())
            || scale.iter().any(|v| v.abs() < 1e-6)
        {
            return None;
        }
        let limit = 2. * layer.pixels.width.max(layer.pixels.height) as f32;
        Some(Self {
            scale: scale.map(|v| v.clamp(-16., 16.)),
            angle: angle.clamp(-20. * std::f32::consts::PI, 20. * std::f32::consts::PI),
            offset: offset.map(|v| v.clamp(-limit, limit)),
        })
    }
}

/// Inverse mapping shared by the edit and the visible preview. The pivot and
/// mesh handles use selection/document coordinates even for sparse layers.
pub struct WarpMap {
    cells: Vec<Cell>,
    bounds: Bounds,
    sampling_bounds: Bounds,
}
impl WarpMap {
    fn bounds(layer: &Layer, selection: &Selection) -> Option<[u32; 4]> {
        let (w, h) = (layer.pixels.width, layer.pixels.height);
        if w == 0 || h == 0 {
            return None;
        }
        if selection.active {
            selection.bounds(w, h)
        } else {
            Some([0, 0, w, h])
        }
    }

    pub fn affine(
        layer: &Layer,
        selection: &Selection,
        scale: [f32; 2],
        angle: f32,
        offset: [f32; 2],
    ) -> Option<Self> {
        let params = AffineTransform::validated(scale, angle, offset, layer)?;
        Self::affine_with_bounds(
            layer,
            selection.active,
            Self::bounds(layer, selection)?,
            params,
        )
    }

    /// Build a preview from bounds captured when the transform gesture began.
    pub fn affine_with_bounds(
        layer: &Layer,
        selected: bool,
        rect: [u32; 4],
        params: AffineTransform,
    ) -> Option<Self> {
        let AffineTransform {
            scale,
            angle,
            offset,
        } = params;
        if scale == [1.; 2] && angle == 0. && offset == [0.; 2] {
            return None;
        }
        let bounds = Bounds::from_rect(rect)?;
        let center = (bounds.min + bounds.max) * 0.5;
        let mut src = bounds;
        if !selected {
            let mut occupied = None;
            for &(tx, ty) in layer
                .pixels
                .tiles
                .keys()
                .chain(layer.mask.iter().flat_map(|m| m.tiles.keys()))
            {
                let p = DVec2::new(
                    f64::from(tx) * f64::from(TILE_SIZE),
                    f64::from(ty) * f64::from(TILE_SIZE),
                );
                let b = occupied.get_or_insert(Bounds::point(p));
                b.add(p);
                b.add(p + f64::from(TILE_SIZE - 1));
            }
            let b = occupied?;
            src.min = (b.min - 1.).max(bounds.min);
            src.max = (b.max + 1.).min(bounds.max);
        }
        let shift = DVec2::from_array(offset.map(f64::from));
        let scale = DVec2::from_array(scale.map(f64::from));
        let quarter = angle / std::f32::consts::FRAC_PI_2;
        let rotation = if quarter.fract() == 0. {
            [DVec2::X, DVec2::Y, -DVec2::X, -DVec2::Y][(quarter as i32).rem_euclid(4) as usize]
        } else {
            DVec2::from_angle(f64::from(angle))
        };
        // A bilinear tap reaches one pixel beyond the source centres.
        src.min -= 1.;
        src.max += 1.;
        let forward = |p: DVec2| center + shift + rotation.rotate((p - center) * scale);
        let corners = [
            src.min,
            DVec2::new(src.max.x, src.min.y),
            DVec2::new(src.min.x, src.max.y),
            src.max,
        ];
        Some(Self {
            bounds,
            sampling_bounds: src,
            cells: vec![Cell::new(src, corners.map(forward))],
        })
    }
    pub fn mesh(
        layer: &Layer,
        selection: &Selection,
        grid: [usize; 2],
        offsets: &[[f32; 2]],
    ) -> Option<Self> {
        Self::mesh_with_bounds(layer, Self::bounds(layer, selection)?, grid, offsets)
    }

    pub fn mesh_with_bounds(
        layer: &Layer,
        rect: [u32; 4],
        [cols, rows]: [usize; 2],
        offsets: &[[f32; 2]],
    ) -> Option<Self> {
        if cols < 2
            || rows < 2
            || cols.checked_mul(rows) != Some(offsets.len())
            || offsets.iter().flatten().any(|v| !v.is_finite())
            || offsets.iter().flatten().all(|v| v.abs() < 0.01)
        {
            return None;
        }
        let bounds = Bounds::from_rect(rect)?;
        let step = (bounds.max - bounds.min).max(DVec2::ONE)
            / DVec2::new((cols - 1) as f64, (rows - 1) as f64);
        let limit = 2. * f64::from(layer.pixels.width.max(layer.pixels.height));
        let src = |x: usize, y: usize| bounds.min + step * DVec2::new(x as f64, y as f64);
        let offset = |x: usize, y: usize| {
            DVec2::from_array(offsets[y * cols + x].map(|v| f64::from(v).clamp(-limit, limit)))
        };
        let mut cells = Vec::new();
        for y in 0..rows - 1 {
            for x in 0..cols - 1 {
                // Continue the edge cell's own bilinear mapping through the
                // one-pixel support of the interpolation kernel.
                let lo = DVec2::new(
                    if x == 0 { -1. / step.x } else { 0. },
                    if y == 0 { -1. / step.y } else { 0. },
                );
                let hi = DVec2::new(
                    if x == cols - 2 { 1. + 1. / step.x } else { 1. },
                    if y == rows - 2 { 1. + 1. / step.y } else { 1. },
                );
                let a = offset(x, y);
                let b = offset(x + 1, y);
                let c = offset(x, y + 1);
                let d = offset(x + 1, y + 1);
                let forward = |uv: DVec2| {
                    src(x, y)
                        + uv * step
                        + a
                        + uv.x * (b - a)
                        + uv.y * (c - a)
                        + uv.x * uv.y * (a - b - c + d)
                };
                let corners = [lo, DVec2::new(hi.x, lo.y), DVec2::new(lo.x, hi.y), hi];
                cells.push(Cell::new(
                    Bounds {
                        min: src(x, y) + lo * step,
                        max: src(x, y) + hi * step,
                    },
                    corners.map(forward),
                ));
            }
        }
        Some(Self {
            cells,
            bounds,
            sampling_bounds: Bounds {
                min: bounds.min - 1.,
                max: bounds.max + 1.,
            },
        })
    }
    fn inverse(&self, cell: &Cell, p: DVec2) -> Option<DVec2> {
        cell.inverse(p)
            .filter(|q| self.sampling_bounds.contains(*q))
            .map(|q| q.clamp(self.sampling_bounds.min, self.sampling_bounds.max))
    }
    fn sample(
        layer: &Layer,
        selection: &Selection,
        x: u32,
        y: u32,
        source: Option<DVec2>,
    ) -> ([u8; 4], u8, u8) {
        let w = layer.pixels.width;
        let h = layer.pixels.height;
        let old = layer.pixels.pixel(x, y);
        let mask = layer
            .mask
            .as_ref()
            .map_or(255, |m| m.pixel_or_tile_default(x, y, [255; 4])[0]);
        let selected = selection.active.then_some(selection.mask.as_slice());
        let cut = selected.map_or(255, |s| s.get((y * w + x) as usize).copied().unwrap_or(0));
        let retained = 1. - f64::from(cut) / 255.;
        if let Some(p) = source {
            // An unchanged tap restores the exact stored bytes, including
            // straight RGB underneath zero alpha.
            if p.abs_diff_eq(DVec2::new(x.into(), y.into()), 1e-10) {
                return (old, mask, cut);
            }
            let sample =
                bilinear_warp_sample(&layer.pixels, layer.mask.as_ref(), selected, w, h, p.x, p.y);
            let coverage = (sample.coverage * 255.).round() as u8;
            if coverage != 0 {
                let (pixel, m) = composite_warp_sample(sample, old, mask, retained);
                return (pixel, m, coverage);
            }
        }
        let mut pixel = old;
        pixel[3] = (f64::from(old[3]) * retained).round() as u8;
        if cut > 0 && pixel[3] == 0 {
            pixel = [0; 4];
        }
        (pixel, if cut == 255 { 255 } else { mask }, 0)
    }
    pub fn preview_pixel(
        &self,
        layer: &Layer,
        selection: &Selection,
        x: u32,
        y: u32,
    ) -> ([u8; 4], u8) {
        let p = DVec2::new(x.into(), y.into());
        let source = self
            .cells
            .iter()
            .rev()
            .find_map(|cell| self.inverse(cell, p));
        let (pixel, mask, _) = Self::sample(layer, selection, x, y, source);
        (pixel, mask)
    }
    pub(super) fn apply(
        &self,
        layer: &mut Layer,
        selection: &mut Selection,
        history: &mut History,
    ) {
        let (w, h) = (layer.pixels.width, layer.pixels.height);
        let size = DVec2::new(w.into(), h.into());
        let empty = || TilePixels::new(w, h);
        let mut pixels = if selection.active {
            layer.pixels.clone()
        } else {
            empty()
        };
        let mut mask = layer
            .mask
            .as_ref()
            .map(|m| if selection.active { m.clone() } else { empty() });
        let mut next = if selection.active {
            vec![0; selection.mask.len()]
        } else {
            Vec::new()
        };
        let mut write = |x, y, src| {
            let (p, m, coverage) = Self::sample(layer, selection, x, y, src);
            if pixels.pixel(x, y) != p {
                pixels.set_pixel(x, y, p);
            }
            if let Some(mask) = &mut mask
                && mask.pixel_or_tile_default(x, y, [255; 4])[0] != m
            {
                mask.set_mask_pixel(x, y, [m; 4]);
            }
            if selection.active {
                next[(y * w + x) as usize] = coverage;
            }
        };
        if selection.active {
            for (x, y) in self.bounds.pixels(size) {
                if selection.mask[(y * w + x) as usize] != 0 {
                    write(x, y, None);
                }
            }
        }
        for cell in &self.cells {
            for (x, y) in cell.dest.pixels(size) {
                if let Some(src) = self.inverse(cell, DVec2::new(x.into(), y.into())) {
                    write(x, y, Some(src));
                }
            }
        }
        pixels.prune_empty_tiles();
        for (x, y) in pixels.tiles_changed_since(&layer.pixels) {
            history.record_tile(layer, x, y);
        }
        if let (Some(m), Some(old)) = (&mut mask, &layer.mask) {
            m.prune_white_tiles();
            for (x, y) in m.tiles_changed_since(old) {
                history.record_mask(layer, ((y * TILE_SIZE * w + x * TILE_SIZE) * 4) as usize);
            }
        }
        layer.pixels = pixels;
        layer.mask = mask;
        if selection.active {
            selection.mask = next;
        }
    }
}
