// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use efude_canvas::{
    BlendMode, Document, GuideImage, Layer, LayerKind, MAX_DOCUMENT_DIMENSION, MAX_DOCUMENT_PIXELS,
    TILE_SIZE, composite, composite_transparent, sparse_tile_keys,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    fs::{self, File},
    io::{Read, Seek, Write},
    path::Path,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};
pub mod timelapse;
const MAX_EFUDE_TILE_MEMORY: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EFUDE_LAYER_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EFUDE_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EFUDE_VECTOR_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Copy)]
struct EfudeLimits {
    vector_bytes: u64,
    archive_entries: u64,
    tile_slots: u64,
    decoded_tile_memory: u64,
}
const EFUDE_LIMITS: EfudeLimits = EfudeLimits {
    vector_bytes: MAX_EFUDE_VECTOR_BYTES,
    archive_entries: 500_000,
    tile_slots: 2_000_000,
    decoded_tile_memory: MAX_EFUDE_TILE_MEMORY,
};
const MAX_INCREMENTAL_ENTRY_COMPARE: usize = 32 * 1024 * 1024;
// Backing tile buffers of editable PSD layers; not total process memory.
const MAX_PSD_TILE_MEMORY: u64 = 512 * 1024 * 1024;

fn write_or_reuse_entry<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    previous: &mut Option<ZipArchive<File>>,
    name: &str,
    options: SimpleFileOptions,
    data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let can_reuse = if data.len() <= MAX_INCREMENTAL_ENTRY_COMPARE {
        if let Some(archive) = previous.as_mut() {
            if let Ok(mut old_entry) = archive.by_name(name) {
                if old_entry.size() == data.len() as u64 {
                    let mut old_data = Vec::with_capacity(data.len());
                    old_entry.read_to_end(&mut old_data).is_ok() && old_data == data
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    if can_reuse {
        writer.raw_copy_file(previous.as_mut().unwrap().by_name(name)?)?;
        return Ok(());
    }

    writer.start_file(name, options)?;
    writer.write_all(data)?;
    Ok(())
}

fn write_or_reuse_tile<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    previous: &mut Option<ZipArchive<File>>,
    name: &str,
    options: SimpleFileOptions,
    pixels: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let can_reuse = if let Some(archive) = previous.as_mut() {
        if let Ok(mut old_entry) = archive.by_name(name) {
            if old_entry.size() >= 4 && old_entry.size() <= pixels.len() as u64 + 64 {
                let mut old_tile = Vec::with_capacity(old_entry.size() as usize);
                old_entry.read_to_end(&mut old_tile).is_ok()
                    && old_tile.len() >= 4
                    && u32::from_le_bytes(old_tile[..4].try_into().unwrap()) as usize
                        == pixels.len()
                    && lz4_flex::decompress_size_prepended(&old_tile)
                        .is_ok_and(|decoded| decoded == pixels)
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    if can_reuse {
        writer.raw_copy_file(previous.as_mut().unwrap().by_name(name)?)?;
        return Ok(());
    }

    writer.start_file(name, options)?;
    writer.write_all(&lz4_flex::compress_prepend_size(pixels))?;
    Ok(())
}

fn optional_entry<'a, R: Read + Seek>(
    archive: &'a mut ZipArchive<R>,
    name: &str,
) -> Result<Option<zip::read::ZipFile<'a>>, zip::result::ZipError> {
    match archive.by_name(name) {
        Ok(entry) => Ok(Some(entry)),
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn reserve_efude_tile_memory(used: &mut u64, limit: u64) -> Result<(), Box<dyn std::error::Error>> {
    let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
    *used = used
        .checked_add(tile_bytes)
        .ok_or(".efude decoded tile memory size overflow")?;
    if *used > limit {
        return Err(".efude decoded tiles exceed the 2 GiB memory limit".into());
    }
    Ok(())
}

fn validate_tile_slots(
    width: u32,
    height: u32,
    layer_count: u64,
    limit: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let tile_slots = u64::from(width.div_ceil(TILE_SIZE))
        .checked_mul(u64::from(height.div_ceil(TILE_SIZE)))
        .and_then(|slots| slots.checked_mul(layer_count))
        .ok_or(".efude canvas and layer count overflow")?;
    if tile_slots > limit {
        return Err(".efude canvas and layer count exceed loading limits".into());
    }
    Ok(())
}

fn uniform_mask_value(data: &[u8]) -> Option<u8> {
    let &value = data.first()?;
    data.iter().all(|&byte| byte == value).then_some(value)
}

fn validate_vector_strokes(
    strokes: &[efude_canvas::VectorStroke],
) -> Result<(), Box<dyn std::error::Error>> {
    if strokes.iter().any(|stroke| {
        !stroke.hardness.is_finite()
            || stroke
                .points
                .iter()
                .any(|p| !p.x.is_finite() || !p.y.is_finite() || !p.width.is_finite())
            || stroke.anchors.iter().any(|anchor| {
                [
                    anchor.x,
                    anchor.y,
                    anchor.width,
                    anchor.in_x,
                    anchor.in_y,
                    anchor.out_x,
                    anchor.out_y,
                ]
                .iter()
                .any(|value| !value.is_finite())
            })
    }) {
        return Err(".efude vector stroke has invalid geometry or hardness".into());
    }
    Ok(())
}

fn validate_entry_size(
    bytes: u64,
    limit: u64,
    error: &'static str,
) -> Result<(), Box<dyn std::error::Error>> {
    if bytes > limit {
        return Err(error.into());
    }
    Ok(())
}

pub fn save(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    save_with_limits(path, doc, EFUDE_LIMITS)
}

fn save_with_limits(
    path: &Path,
    doc: &Document,
    limits: EfudeLimits,
) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err(".efude canvas dimensions exceed safety limits".into());
    }
    if !doc.dpi.is_finite() || doc.dpi <= 0.0 || doc.dpi > 10_000.0 {
        return Err(".efude DPI is invalid".into());
    }
    if doc.layers.is_empty() || doc.layers.len() > 2000 {
        return Err(".efude layer count is outside the supported range".into());
    }
    validate_tile_slots(
        doc.width,
        doc.height,
        doc.layers.len() as u64,
        limits.tile_slots,
    )?;
    if doc.guide.as_ref().is_some_and(|guide| !guide.is_valid()) {
        return Err(".efude tracing guide is invalid".into());
    }
    let mut ids = std::collections::HashSet::with_capacity(doc.layers.len());
    for layer in &doc.layers {
        if let Some(strokes) = &layer.vector {
            validate_vector_strokes(strokes)?;
        }
        if !ids.insert(layer.id) {
            return Err(".efude contains duplicate layer IDs".into());
        }
        if let Some(parent_id) = layer.parent_id {
            let parent = doc
                .layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .ok_or(".efude layer references a missing parent")?;
            if parent.kind != LayerKind::Folder {
                return Err(".efude layer parent is not a folder".into());
            }
        }
        let mut current = layer.parent_id;
        let mut ancestors = std::collections::HashSet::new();
        while let Some(parent_id) = current {
            if !ancestors.insert(parent_id) || parent_id == layer.id {
                return Err(".efude folder hierarchy contains a cycle".into());
            }
            current = doc
                .layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|parent| parent.parent_id);
        }
    }
    // mimetype, manifest, layers, merged image, thumbnail, optional metadata,
    // guide settings/image, and each layer's vector data/paint tiles/mask tiles.
    let mut entry_count =
        5u64 + u64::from(!doc.metadata.is_empty()) + 2 * u64::from(doc.guide.is_some());
    for layer in &doc.layers {
        entry_count = entry_count
            .checked_add(u64::from(layer.vector.is_some()))
            .and_then(|count| {
                count.checked_add(sparse_tile_keys(layer, doc.width, doc.height).len() as u64)
            })
            .ok_or(".efude archive entry count overflow")?;
        if let Some(mask) = &layer.mask {
            entry_count = entry_count
                .checked_add(
                    mask.tile_keys()
                        .into_iter()
                        .filter(|&(x, y)| x * TILE_SIZE < doc.width && y * TILE_SIZE < doc.height)
                        .count() as u64,
                )
                .ok_or(".efude archive entry count overflow")?;
        }
    }
    if entry_count > limits.archive_entries {
        return Err(".efude archive would contain too many entries".into());
    }
    let mut previous_archive = File::open(path)
        .ok()
        .and_then(|file| ZipArchive::new(file).ok())
        .filter(|archive| archive.len() as u64 <= limits.archive_entries);
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let f = temp.as_file().try_clone()?;
    let mut z = ZipWriter::new(f);
    let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let tile_options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    write_or_reuse_entry(
        &mut z,
        &mut previous_archive,
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        b"application/x-efude",
    )?;
    let manifest = serde_json::to_vec_pretty(
        &serde_json::json!({"format_version":"0.1","width":doc.width,"height":doc.height,"dpi":doc.dpi,"color_space":"sRGB","pixel_format":"RGBA8"}),
    )?;
    write_or_reuse_entry(&mut z, &mut previous_archive, "manifest.json", o, &manifest)?;
    let layers = serde_json::to_vec_pretty(&doc.layers.iter().map(|l|serde_json::json!({"id":l.id,"name":l.name,"visible":l.visible,"opacity":l.opacity,"locked":l.locked,"clipping":l.clipping,"sketch":l.sketch,"reference":l.reference,"blend":l.blend,"linear_blend":l.linear_blend,"kind":l.kind,"parent_id":l.parent_id,"expanded":l.expanded,"has_mask":l.mask.is_some(),"tone":l.tone})).collect::<Vec<_>>())?;
    validate_entry_size(
        layers.len() as u64,
        MAX_EFUDE_LAYER_METADATA_BYTES,
        ".efude layer metadata exceeds the size limit",
    )?;
    write_or_reuse_entry(&mut z, &mut previous_archive, "layers.json", o, &layers)?;
    if !doc.metadata.is_empty() {
        let metadata = serde_json::to_vec_pretty(&doc.metadata)?;
        validate_entry_size(
            metadata.len() as u64,
            MAX_EFUDE_METADATA_BYTES,
            ".efude metadata exceeds the size limit",
        )?;
        write_or_reuse_entry(&mut z, &mut previous_archive, "metadata.json", o, &metadata)?;
    }
    if let Some(guide) = &doc.guide {
        let settings = serde_json::to_vec_pretty(&serde_json::json!({
            "width": guide.width, "height": guide.height,
            "offset_x": guide.offset_x, "offset_y": guide.offset_y,
            "scale": guide.scale, "opacity": guide.opacity, "visible": guide.visible
        }))?;
        write_or_reuse_entry(
            &mut z,
            &mut previous_archive,
            "guide/settings.json",
            o,
            &settings,
        )?;
        let mut bytes = Vec::new();
        {
            let mut encoder =
                png::Encoder::new(std::io::Cursor::new(&mut bytes), guide.width, guide.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&guide.rgba)?;
        }
        write_or_reuse_entry(&mut z, &mut previous_archive, "guide/image.png", o, &bytes)?;
    }
    let mut allocated_tile_memory = 0u64;
    for l in &doc.layers {
        if let Some(strokes) = &l.vector {
            let bytes = serde_json::to_vec(strokes)?;
            validate_entry_size(
                bytes.len() as u64,
                limits.vector_bytes,
                ".efude vector strokes exceed the size limit",
            )?;
            write_or_reuse_entry(
                &mut z,
                &mut previous_archive,
                &format!("vectors/{}.json", l.id),
                o,
                &bytes,
            )?;
        }
        for (tx, ty) in sparse_tile_keys(l, doc.width, doc.height) {
            reserve_efude_tile_memory(&mut allocated_tile_memory, limits.decoded_tile_memory)?;
            let tw = TILE_SIZE.min(doc.width - tx * TILE_SIZE);
            let th = TILE_SIZE.min(doc.height - ty * TILE_SIZE);
            let mut tile = Vec::with_capacity((tw * th * 4) as usize);
            for y in ty * TILE_SIZE..ty * TILE_SIZE + th {
                let start = ((y * doc.width + tx * TILE_SIZE) * 4) as usize;
                // Store the in-memory straight-alpha RGBA8 bytes unchanged so a
                // save/load round trip is lossless (see docs/spec/efude-format.md).
                tile.extend_from_slice(&l.pixels[start..start + (tw * 4) as usize]);
            }
            write_or_reuse_tile(
                &mut z,
                &mut previous_archive,
                &format!("tiles/{}/{tx}_{ty}.bin", l.id),
                tile_options,
                &tile,
            )?;
        }
        if let Some(mask) = &l.mask {
            for (tx, ty) in mask.tile_keys() {
                if tx * TILE_SIZE >= doc.width || ty * TILE_SIZE >= doc.height {
                    continue;
                }
                let tw = TILE_SIZE.min(doc.width - tx * TILE_SIZE);
                let th = TILE_SIZE.min(doc.height - ty * TILE_SIZE);
                let mut tile = Vec::with_capacity((tw * th) as usize);
                for y in 0..th {
                    for x in 0..tw {
                        tile.push(mask.pixel(tx * TILE_SIZE + x, ty * TILE_SIZE + y)[0]);
                    }
                }
                if uniform_mask_value(&tile).is_none() {
                    reserve_efude_tile_memory(
                        &mut allocated_tile_memory,
                        limits.decoded_tile_memory,
                    )?;
                }
                write_or_reuse_entry(
                    &mut z,
                    &mut previous_archive,
                    &format!("masks/{}/{tx}_{ty}.gray", l.id),
                    o,
                    &tile,
                )?;
            }
        }
    }
    let merged_pixels = composite(doc);
    let mut png_bytes = Vec::new();
    {
        let mut e = png::Encoder::new(std::io::Cursor::new(&mut png_bytes), doc.width, doc.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&merged_pixels)?;
    }
    write_or_reuse_entry(&mut z, &mut previous_archive, "merged.png", o, &png_bytes)?;
    let full_preview = image::RgbaImage::from_raw(doc.width, doc.height, merged_pixels)
        .ok_or("could not create document preview")?;
    let thumbnail = image::imageops::thumbnail(&full_preview, 256, 256);
    let mut thumbnail_bytes = Vec::new();
    {
        let mut e = png::Encoder::new(
            std::io::Cursor::new(&mut thumbnail_bytes),
            thumbnail.width(),
            thumbnail.height(),
        );
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(thumbnail.as_raw())?;
    }
    write_or_reuse_entry(
        &mut z,
        &mut previous_archive,
        "thumbnail.png",
        o,
        &thumbnail_bytes,
    )?;
    let file = z.finish()?;
    file.sync_all()?;
    drop(file);
    // Windows cannot replace a file that is still open: close the old
    // archive (read for unchanged entries) first.
    drop(previous_archive);
    temp.persist(path)?;
    Ok(())
}
pub fn load(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
    load_with_limits(path, EFUDE_LIMITS)
}

fn load_with_limits(
    path: &Path,
    limits: EfudeLimits,
) -> Result<Document, Box<dyn std::error::Error>> {
    use std::io::Read;
    let f = File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    if z.len() as u64 > limits.archive_entries {
        return Err(".efude archive contains too many entries".into());
    }
    {
        let mut entry = z.by_index(0)?;
        if entry.name() != "mimetype" || entry.compression() != zip::CompressionMethod::Stored {
            return Err(
                "invalid .efude container: mimetype must be the first uncompressed entry".into(),
            );
        }
        let expected = b"application/x-efude";
        if entry.size() != expected.len() as u64 {
            return Err("invalid .efude mimetype length".into());
        }
        let mut mime = vec![0; expected.len()];
        entry.read_exact(&mut mime)?;
        if mime != expected {
            return Err("invalid .efude mimetype".into());
        }
    }
    let mut manifest_entry = z.by_name("manifest.json")?;
    if manifest_entry.size() > 1024 * 1024 {
        return Err(".efude manifest exceeds the size limit".into());
    }
    let mut manifest_bytes = Vec::with_capacity(manifest_entry.size() as usize);
    manifest_entry.read_to_end(&mut manifest_bytes)?;
    drop(manifest_entry);
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)?;
    let version = manifest["format_version"]
        .as_str()
        .ok_or(".efude format_version is missing")?;
    if version != "0.1" {
        return Err(format!("unsupported .efude format version: {version}").into());
    }
    if manifest["color_space"].as_str() != Some("sRGB")
        || !matches!(
            manifest["pixel_format"].as_str(),
            Some("RGBA8") | Some("RGBA8_PREMULTIPLIED")
        )
    {
        return Err("unsupported .efude color space or pixel format".into());
    }
    let width = u32::try_from(
        manifest["width"]
            .as_u64()
            .ok_or(".efude width is missing")?,
    )?;
    let height = u32::try_from(
        manifest["height"]
            .as_u64()
            .ok_or(".efude height is missing")?,
    )?;
    let premultiplied = manifest["pixel_format"].as_str() == Some("RGBA8_PREMULTIPLIED");
    if !efude_canvas::valid_document_dimensions(width, height) {
        return Err(".efude canvas dimensions exceed safety limits".into());
    }
    let dpi = manifest["dpi"].as_f64().unwrap_or(300.) as f32;
    if !dpi.is_finite() || dpi <= 0. || dpi > 10000. {
        return Err(".efude DPI is invalid".into());
    }
    let mut layers_entry = z.by_name("layers.json")?;
    validate_entry_size(
        layers_entry.size(),
        MAX_EFUDE_LAYER_METADATA_BYTES,
        ".efude layer metadata exceeds the size limit",
    )?;
    let mut layers_bytes = Vec::with_capacity(layers_entry.size() as usize);
    layers_entry.read_to_end(&mut layers_bytes)?;
    drop(layers_entry);
    let metas: Vec<serde_json::Value> = serde_json::from_slice(&layers_bytes)?;
    if metas.is_empty() || metas.len() > 2000 {
        return Err(".efude layer count is outside the supported range".into());
    }
    validate_tile_slots(width, height, metas.len() as u64, limits.tile_slots)?;
    let mut layers = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut allocated_tile_memory = 0u64;
    let mut uniform_mask_tiles = std::collections::HashMap::<u8, std::sync::Arc<Vec<u8>>>::new();
    for meta in metas {
        let id = meta["id"].as_u64().unwrap_or(layers.len() as u64 + 1);
        if !ids.insert(id) {
            return Err(".efude contains duplicate layer IDs".into());
        }
        let name = meta["name"].as_str().unwrap_or("レイヤー").to_string();
        let mut l = Layer::new(id, name, width, height);
        l.visible = meta["visible"].as_bool().unwrap_or(true);
        l.opacity = (meta["opacity"].as_f64().unwrap_or(1.) as f32).clamp(0., 1.);
        l.locked = meta["locked"].as_bool().unwrap_or(false);
        l.clipping = meta["clipping"].as_bool().unwrap_or(false);
        l.sketch = meta["sketch"].as_bool().unwrap_or(false);
        l.reference = meta["reference"].as_bool().unwrap_or(false);
        l.blend = serde_json::from_value(meta["blend"].clone()).unwrap_or(BlendMode::Normal);
        l.linear_blend = meta["linear_blend"].as_bool().unwrap_or(false);
        l.kind = serde_json::from_value(meta["kind"].clone()).unwrap_or(LayerKind::Raster);
        l.parent_id = meta["parent_id"].as_u64();
        l.expanded = meta["expanded"].as_bool().unwrap_or(true);
        l.tone = serde_json::from_value(meta["tone"].clone()).unwrap_or(None);
        if meta["has_mask"].as_bool().unwrap_or(false) {
            l.mask = Some(efude_canvas::TilePixels::new(width, height));
        }
        if let Some(mut entry) = optional_entry(&mut z, &format!("vectors/{id}.json"))? {
            validate_entry_size(
                entry.size(),
                limits.vector_bytes,
                ".efude vector strokes exceed the size limit",
            )?;
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes)?;
            let strokes: Vec<efude_canvas::VectorStroke> = serde_json::from_slice(&bytes)?;
            validate_vector_strokes(&strokes)?;
            l.vector = Some(strokes);
        }
        for ty in 0..height.div_ceil(TILE_SIZE) {
            for tx in 0..width.div_ceil(TILE_SIZE) {
                let tw = TILE_SIZE.min(width - tx * TILE_SIZE);
                let th = TILE_SIZE.min(height - ty * TILE_SIZE);
                let expected = (tw * th * 4) as usize;
                let compressed_name = format!("tiles/{id}/{tx}_{ty}.bin");
                let tile = if let Some(mut entry) = optional_entry(&mut z, &compressed_name)? {
                    let max_packed = expected.saturating_add(65_536);
                    if entry.size() < 4 || entry.size() > max_packed as u64 {
                        return Err(".efude compressed tile exceeds the size limit".into());
                    }
                    let mut packed = Vec::with_capacity(entry.size() as usize);
                    (&mut entry)
                        .take(max_packed as u64 + 1)
                        .read_to_end(&mut packed)?;
                    if packed.len() < 4
                        || packed.len() > max_packed
                        || u32::from_le_bytes(packed[..4].try_into()?) as usize != expected
                    {
                        return Err(".efude tile has an invalid LZ4 size header".into());
                    }
                    let mut decoded = vec![0; expected];
                    let written = lz4_flex::block::decompress_into(&packed[4..], &mut decoded)?;
                    if written != expected {
                        return Err(".efude tile has an invalid LZ4 payload".into());
                    }
                    Some(decoded)
                } else if let Some(mut entry) =
                    optional_entry(&mut z, &format!("tiles/{id}/{tx}_{ty}.rgba"))?
                {
                    if entry.size() != expected as u64 {
                        return Err(".efude legacy tile has an invalid size".into());
                    }
                    let mut decoded = vec![0; expected];
                    entry.read_exact(&mut decoded)?;
                    Some(decoded)
                } else {
                    None
                };
                if let Some(mut tile) = tile {
                    reserve_efude_tile_memory(
                        &mut allocated_tile_memory,
                        limits.decoded_tile_memory,
                    )?;
                    if premultiplied {
                        for pixel in tile.chunks_exact_mut(4) {
                            let alpha = pixel[3] as u16;
                            if alpha == 0 {
                                pixel[..3].fill(0);
                            } else if alpha < 255 {
                                for channel in &mut pixel[..3] {
                                    *channel =
                                        ((*channel as u32 * 255 + alpha as u32 / 2) / alpha as u32)
                                            .min(255) as u8;
                                }
                            }
                        }
                    }
                    for y in 0..th {
                        let dst = (((ty * TILE_SIZE + y) * width + tx * TILE_SIZE) * 4) as usize;
                        let src = (y * tw * 4) as usize;
                        l.pixels[dst..dst + (tw * 4) as usize]
                            .copy_from_slice(&tile[src..src + (tw * 4) as usize]);
                    }
                }
            }
        }
        if let Some(mask) = &mut l.mask {
            for ty in 0..height.div_ceil(TILE_SIZE) {
                for tx in 0..width.div_ceil(TILE_SIZE) {
                    let tw = TILE_SIZE.min(width - tx * TILE_SIZE);
                    let th = TILE_SIZE.min(height - ty * TILE_SIZE);
                    if let Some(mut entry) =
                        optional_entry(&mut z, &format!("masks/{id}/{tx}_{ty}.gray"))?
                    {
                        let expected = (tw * th) as usize;
                        if entry.size() != expected as u64 {
                            return Err(".efude mask tile has an invalid size".into());
                        }
                        let mut data = vec![0; expected];
                        entry.read_exact(&mut data)?;
                        // Uniform tiles (the inside and outside of a panel
                        // mask) share one allocation.
                        if let Some(v) = uniform_mask_value(&data) {
                            let tile = uniform_mask_tiles
                                .entry(v)
                                .or_insert_with(|| {
                                    std::sync::Arc::new(
                                        [v, v, v, 255].repeat((TILE_SIZE * TILE_SIZE) as usize),
                                    )
                                })
                                .clone();
                            mask.insert_shared_tile(tx, ty, tile);
                            continue;
                        }
                        reserve_efude_tile_memory(
                            &mut allocated_tile_memory,
                            limits.decoded_tile_memory,
                        )?;
                        mask.ensure_tile_filled(tx * TILE_SIZE, ty * TILE_SIZE, [255; 4]);
                        for y in 0..th {
                            for x in 0..tw {
                                let v = data[(y * tw + x) as usize];
                                mask.set_pixel(
                                    tx * TILE_SIZE + x,
                                    ty * TILE_SIZE + y,
                                    [v, v, v, 255],
                                );
                            }
                        }
                    }
                }
            }
        }
        layers.push(l);
    }
    for layer in &layers {
        if let Some(parent_id) = layer.parent_id {
            let parent = layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .ok_or(".efude layer references a missing parent")?;
            if parent.kind != LayerKind::Folder {
                return Err(".efude layer parent is not a folder".into());
            }
        }
        let mut current = layer.parent_id;
        let mut ancestors = std::collections::HashSet::new();
        while let Some(parent_id) = current {
            if !ancestors.insert(parent_id) || parent_id == layer.id {
                return Err(".efude folder hierarchy contains a cycle".into());
            }
            current = layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|p| p.parent_id);
        }
    }
    let mut metadata = std::collections::BTreeMap::new();
    if let Some(mut entry) = optional_entry(&mut z, "metadata.json")? {
        validate_entry_size(
            entry.size(),
            MAX_EFUDE_METADATA_BYTES,
            ".efude metadata exceeds the size limit",
        )?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        metadata = serde_json::from_slice(&bytes)?;
    }
    let has_guide = z.file_names().any(|name| name == "guide/settings.json");
    let guide = if has_guide {
        let mut settings_entry = z.by_name("guide/settings.json")?;
        if settings_entry.size() > 64 * 1024 {
            return Err(".efude guide settings exceed the size limit".into());
        }
        let mut settings_bytes = Vec::new();
        settings_entry.read_to_end(&mut settings_bytes)?;
        drop(settings_entry);
        let settings: serde_json::Value = serde_json::from_slice(&settings_bytes)?;
        let guide_width = u32::try_from(
            settings["width"]
                .as_u64()
                .ok_or(".efude guide width is missing")?,
        )?;
        let guide_height = u32::try_from(
            settings["height"]
                .as_u64()
                .ok_or(".efude guide height is missing")?,
        )?;
        if guide_width == 0
            || guide_height == 0
            || u64::from(guide_width) * u64::from(guide_height) > 16_777_216
        {
            return Err(".efude guide dimensions exceed the size limit".into());
        }
        let mut image_entry = z.by_name("guide/image.png")?;
        if image_entry.size() > 128 * 1024 * 1024 {
            return Err(".efude guide image exceeds the size limit".into());
        }
        let mut image_bytes = Vec::new();
        image_entry.read_to_end(&mut image_bytes)?;
        let decoder = png::Decoder::new(std::io::Cursor::new(&image_bytes));
        let mut reader = decoder.read_info()?;
        if reader.info().width != guide_width
            || reader.info().height != guide_height
            || reader.info().color_type != png::ColorType::Rgba
            || reader.info().bit_depth != png::BitDepth::Eight
        {
            return Err(".efude guide image format does not match its settings".into());
        }
        let mut rgba = vec![0; guide_width as usize * guide_height as usize * 4];
        let frame = reader.next_frame(&mut rgba)?;
        if frame.buffer_size() != rgba.len() {
            return Err(".efude guide image is incomplete".into());
        }
        let guide = GuideImage {
            width: guide_width,
            height: guide_height,
            rgba: std::sync::Arc::new(rgba),
            offset_x: settings["offset_x"]
                .as_f64()
                .ok_or(".efude guide offset is missing")? as f32,
            offset_y: settings["offset_y"]
                .as_f64()
                .ok_or(".efude guide offset is missing")? as f32,
            scale: settings["scale"]
                .as_f64()
                .ok_or(".efude guide scale is missing")? as f32,
            opacity: settings["opacity"]
                .as_f64()
                .ok_or(".efude guide opacity is missing")? as f32,
            visible: settings["visible"]
                .as_bool()
                .ok_or(".efude guide visibility is missing")?,
        };
        if !guide.is_valid() {
            return Err(".efude guide settings are invalid".into());
        }
        Some(guide)
    } else {
        None
    };
    efude_canvas::tidy_layer_order(&mut layers);
    Ok(Document {
        width,
        height,
        dpi,
        layers,
        guide,
        metadata,
    })
}
/// The preview stored in an `.efude` file (at most 256×256), as
/// straight-alpha RGBA, without loading the document.
pub fn load_thumbnail(path: &Path) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let mut entry = archive.by_name("thumbnail.png")?;
    if entry.size() > 4 * 1024 * 1024 {
        return Err("thumbnail too large".into());
    }
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    Ok((image.width(), image.height(), image.into_raw()))
}

pub fn export_png(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err("image dimensions exceed safety limits".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut e = png::Encoder::new(temp.reopen()?, doc.width, doc.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?
            .write_image_data(&composite_transparent(doc))?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn export_jpeg(
    path: &Path,
    doc: &Document,
    quality: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err("image dimensions exceed safety limits".into());
    }
    let rgba = composite(doc);
    let mut rgb = Vec::with_capacity((doc.width * doc.height * 3) as usize);
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(temp.reopen()?, quality.clamp(1, 100))
        .encode(&rgb, doc.width, doc.height, image::ExtendedColorType::Rgb8)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn export_psd(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    export_psd_report(path, doc).map(|_| ())
}

pub fn export_psd_report(
    path: &Path,
    doc: &Document,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    const MAX_PSD_LAYERS: usize = 200;
    // PSD has no tone layers: write them as the dots they show.
    let baked;
    let doc = if doc.layers.iter().any(|layer| layer.tone.is_some()) {
        baked = efude_canvas::bake_tone_layers(doc);
        &baked
    } else {
        doc
    };
    if doc.width > MAX_DOCUMENT_DIMENSION || doc.height > MAX_DOCUMENT_DIMENSION {
        return Err("PSD dimensions cannot exceed the supported maximum".into());
    }
    let pixel_count = (doc.width as u64)
        .checked_mul(doc.height as u64)
        .ok_or("PSD dimensions overflow")?;
    if pixel_count == 0 || pixel_count > MAX_DOCUMENT_PIXELS {
        return Err("PSD image exceeds the 100 million pixel safety limit".into());
    }
    let image = composite(doc);
    struct PsdLayer<'a> {
        layer: Option<&'a Layer>,
        section: Option<u32>,
        name: String,
    }
    fn flatten<'a>(layers: &'a [Layer], parent: Option<u64>, out: &mut Vec<PsdLayer<'a>>) {
        for layer in layers.iter().rev().filter(|l| l.parent_id == parent) {
            if layer.kind == LayerKind::Folder {
                out.push(PsdLayer {
                    layer: Some(layer),
                    section: Some(if layer.expanded { 1 } else { 2 }),
                    name: layer.name.clone(),
                });
                flatten(layers, Some(layer.id), out);
                out.push(PsdLayer {
                    layer: None,
                    section: Some(3),
                    name: String::new(),
                });
            } else {
                out.push(PsdLayer {
                    layer: Some(layer),
                    section: None,
                    name: layer.name.clone(),
                });
            }
        }
    }
    let mut ps_layers = Vec::new();
    flatten(&doc.layers, None, &mut ps_layers);
    if doc.layers.len() > MAX_PSD_LAYERS {
        return Err("PSD export supports documents with at most 200 layers".into());
    }
    if ps_layers.len() > MAX_PSD_LAYERS * 2 {
        return Err("PSD folder structure exceeds the supported layer-record limit".into());
    }
    fn encode_packbits(row: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < row.len() {
            let mut run = 1;
            while i + run < row.len() && row[i + run] == row[i] && run < 128 {
                run += 1;
            }
            if run >= 3 {
                out.push((1i16 - run as i16) as u8);
                out.push(row[i]);
                i += run;
            } else {
                let start = i;
                i += run;
                while i < row.len() && i - start < 128 {
                    let mut next_run = 1;
                    while i + next_run < row.len() && row[i + next_run] == row[i] && next_run < 128
                    {
                        next_run += 1;
                    }
                    if next_run >= 3 {
                        break;
                    }
                    i += next_run.min(128 - (i - start));
                }
                let len = i - start;
                out.push((len - 1) as u8);
                out.extend_from_slice(&row[start..i]);
            }
        }
    }
    fn encode_channel<F: FnMut(u32, u32) -> u8>(
        width: u32,
        height: u32,
        mut sample: F,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        output.extend_from_slice(&1u16.to_be_bytes());
        let lengths_at = output.len();
        output.resize(lengths_at + height as usize * 2, 0);
        let mut row = vec![0; width as usize];
        for y in 0..height {
            for x in 0..width {
                row[x as usize] = sample(x, y);
            }
            let start = output.len();
            encode_packbits(&row, &mut output);
            let length = output.len() - start;
            if length > u16::MAX as usize {
                return Err("PSD PackBits row exceeds format limits".into());
            }
            output[lengths_at + y as usize * 2..lengths_at + y as usize * 2 + 2]
                .copy_from_slice(&(length as u16).to_be_bytes());
        }
        Ok(output)
    }
    fn encode_layer_channel(
        pixels: &efude_canvas::TilePixels,
        width: u32,
        height: u32,
        channel: usize,
        default: u8,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut tiles_by_row = std::collections::HashMap::<u32, Vec<(u32, &[u8])>>::new();
        for ((tile_x, tile_y), data) in pixels.tiles() {
            tiles_by_row.entry(tile_y).or_default().push((tile_x, data));
        }
        let mut default_row = vec![default; width as usize];
        let mut default_encoded = Vec::new();
        encode_packbits(&default_row, &mut default_encoded);
        let mut output = Vec::new();
        output.extend_from_slice(&1u16.to_be_bytes());
        let lengths_at = output.len();
        output.resize(lengths_at + height as usize * 2, 0);
        for y in 0..height {
            let start = output.len();
            if let Some(tiles) = tiles_by_row.get(&(y / TILE_SIZE)) {
                let row = &mut default_row;
                row.fill(default);
                let local_y = y % TILE_SIZE;
                for &(tile_x, tile) in tiles {
                    let start_x = tile_x * TILE_SIZE;
                    if start_x >= width {
                        continue;
                    }
                    let tile_width = TILE_SIZE.min(width - start_x);
                    for local_x in 0..tile_width {
                        let source = ((local_y * TILE_SIZE + local_x) * 4) as usize + channel;
                        row[(start_x + local_x) as usize] =
                            tile.get(source).copied().unwrap_or(default);
                    }
                }
                encode_packbits(row, &mut output);
            } else {
                output.extend_from_slice(&default_encoded);
            }
            let length = output.len() - start;
            if length > u16::MAX as usize {
                return Err("PSD PackBits row exceeds format limits".into());
            }
            output[lengths_at + y as usize * 2..lengths_at + y as usize * 2 + 2]
                .copy_from_slice(&(length as u16).to_be_bytes());
        }
        Ok(output)
    }
    let mut layer_channels = Vec::with_capacity(ps_layers.len());
    let mut compressed_layer_bytes = 0u64;
    for ps in &ps_layers {
        let Some(layer) = ps.layer else {
            layer_channels.push(Vec::new());
            continue;
        };
        let raster = layer.kind == LayerKind::Raster;
        let mut channels = Vec::with_capacity(if layer.mask.is_some() {
            if raster { 5 } else { 1 }
        } else if raster {
            4
        } else {
            0
        });
        if raster {
            for channel in 0..4 {
                channels.push(encode_layer_channel(
                    &layer.pixels,
                    doc.width,
                    doc.height,
                    channel,
                    0,
                )?);
            }
        }
        if let Some(mask) = &layer.mask {
            channels.push(encode_layer_channel(mask, doc.width, doc.height, 0, 255)?);
        }
        for channel in &channels {
            compressed_layer_bytes = compressed_layer_bytes
                .checked_add(channel.len() as u64)
                .ok_or("PSD compressed layer size overflow")?;
        }
        if compressed_layer_bytes > 1536 * 1024 * 1024 {
            return Err("PSD compressed layer data exceeds the 1.5 GiB safety limit".into());
        }
        layer_channels.push(channels);
    }
    let mut records = Vec::new();
    for (layer_index, ps) in ps_layers.iter().enumerate() {
        let layer = ps.layer;
        let raster = layer.is_some_and(|l| l.kind == LayerKind::Raster);
        for value in if raster {
            [0i32, 0, doc.height as i32, doc.width as i32]
        } else {
            [0i32; 4]
        } {
            records.extend_from_slice(&value.to_be_bytes());
        }
        let channel_count = layer_channels[layer_index].len() as u16;
        records.extend_from_slice(&channel_count.to_be_bytes());
        let channel_ids = if raster {
            [
                Some(0i16),
                Some(1),
                Some(2),
                Some(-1),
                layer.unwrap().mask.as_ref().map(|_| -2),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
        } else {
            layer
                .and_then(|layer| layer.mask.as_ref())
                .map_or_else(Vec::new, |_| vec![-2])
        };
        for (channel_index, id) in channel_ids.into_iter().enumerate() {
            records.extend_from_slice(&id.to_be_bytes());
            let data_bytes = layer_channels[layer_index][channel_index].len();
            if data_bytes > u32::MAX as usize {
                return Err("PSD channel exceeds format size limits".into());
            }
            records.extend_from_slice(&(data_bytes as u32).to_be_bytes());
        }
        records.extend_from_slice(b"8BIM");
        if matches!(ps.section, Some(1 | 2)) {
            records.extend_from_slice(b"pass");
        } else {
            records.extend_from_slice(match layer.map_or(BlendMode::Normal, |l| l.blend) {
                BlendMode::Normal => b"norm",
                BlendMode::Multiply => b"mul ",
                BlendMode::Screen => b"scrn",
                BlendMode::Overlay => b"over",
                BlendMode::Darken => b"dark",
                BlendMode::Lighten => b"lite",
                BlendMode::ColorDodge => b"div ",
                BlendMode::ColorBurn => b"idiv",
                BlendMode::HardLight => b"hLit",
                BlendMode::SoftLight => b"sLit",
                BlendMode::Difference => b"diff",
                BlendMode::Exclusion => b"smud",
                BlendMode::Add => b"lddg",
                BlendMode::Subtract => b"fsub",
            });
        }
        records.push(layer.map_or(255, |l| (l.opacity.clamp(0., 1.) * 255.) as u8));
        records.push(if layer.is_some_and(|l| l.clipping) {
            1
        } else {
            0
        });
        records.push(if layer.is_none_or(|l| l.visible) {
            0
        } else {
            2
        });
        records.push(0);
        let mut extra = Vec::new();
        if layer.is_some_and(|l| l.mask.is_some()) {
            // The mask header is 18 bytes followed by two bytes of padding.
            // Include all 20 bytes in its length so subsequent fields stay aligned.
            extra.extend_from_slice(&20u32.to_be_bytes());
            for v in [0i32, 0, doc.height as i32, doc.width as i32] {
                extra.extend_from_slice(&v.to_be_bytes());
            }
            extra.push(255);
            extra.push(0);
            extra.extend_from_slice(&[0; 2]);
        } else {
            extra.extend_from_slice(&0u32.to_be_bytes());
        }
        extra.extend_from_slice(&0u32.to_be_bytes());
        // The Pascal name is read in the reader's legacy encoding, so it
        // only carries ASCII; the real name goes in the Unicode 'luni' block.
        let ascii_name: Vec<u8> = ps
            .name
            .chars()
            .take(255)
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() {
                    c as u8
                } else {
                    b'?'
                }
            })
            .collect();
        extra.push(ascii_name.len() as u8);
        extra.extend_from_slice(&ascii_name);
        while extra.len() % 4 != 0 {
            extra.push(0)
        }
        let utf16: Vec<u16> = ps.name.encode_utf16().collect();
        let mut unicode_name = Vec::with_capacity(4 + utf16.len() * 2);
        unicode_name.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
        for unit in &utf16 {
            unicode_name.extend_from_slice(&unit.to_be_bytes());
        }
        while unicode_name.len() % 4 != 0 {
            unicode_name.push(0);
        }
        extra.extend_from_slice(b"8BIMluni");
        extra.extend_from_slice(&(unicode_name.len() as u32).to_be_bytes());
        extra.extend_from_slice(&unicode_name);
        if let Some(section) = ps.section {
            extra.extend_from_slice(b"8BIMlsct");
            extra.extend_from_slice(&4u32.to_be_bytes());
            extra.extend_from_slice(&section.to_be_bytes());
        }
        records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        records.extend_from_slice(&extra);
    }
    let channel_data = layer_channels
        .into_iter()
        .flatten()
        .flatten()
        .collect::<Vec<_>>();
    let mut layer_info = Vec::new();
    layer_info.extend_from_slice(&(ps_layers.len() as u16).to_be_bytes());
    layer_info.extend_from_slice(&records);
    layer_info.extend_from_slice(&channel_data);
    if layer_info.len() % 2 != 0 {
        layer_info.push(0)
    }
    let mut layer_mask = Vec::new();
    layer_mask.extend_from_slice(&(layer_info.len() as u32).to_be_bytes());
    layer_mask.extend_from_slice(&layer_info);
    // Empty global layer mask info; no further tagged blocks.
    layer_mask.extend_from_slice(&0u32.to_be_bytes());
    let dpi = if doc.dpi.is_finite() {
        doc.dpi.clamp(1.0, 65535.0)
    } else {
        300.0
    };
    let dpi_fixed = (dpi * 65536.0).round() as u32;
    let mut resolution = Vec::with_capacity(16);
    resolution.extend_from_slice(&dpi_fixed.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes()); // pixels per inch
    resolution.extend_from_slice(&1u16.to_be_bytes()); // display unit: inches
    resolution.extend_from_slice(&dpi_fixed.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes());
    let mut image_resources = Vec::with_capacity(28);
    image_resources.extend_from_slice(b"8BIM");
    image_resources.extend_from_slice(&0x03EDu16.to_be_bytes()); // ResolutionInfo
    image_resources.extend_from_slice(&[0, 0]); // empty Pascal name and even padding
    image_resources.extend_from_slice(&(resolution.len() as u32).to_be_bytes());
    image_resources.extend_from_slice(&resolution);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut f = temp.as_file().try_clone()?;
    f.write_all(b"8BPS")?;
    f.write_all(&1u16.to_be_bytes())?;
    f.write_all(&[0; 6])?;
    f.write_all(&3u16.to_be_bytes())?;
    f.write_all(&doc.height.min(30000).to_be_bytes())?;
    f.write_all(&doc.width.min(30000).to_be_bytes())?;
    f.write_all(&8u16.to_be_bytes())?;
    f.write_all(&3u16.to_be_bytes())?;
    f.write_all(&0u32.to_be_bytes())?;
    f.write_all(&(image_resources.len() as u32).to_be_bytes())?;
    f.write_all(&image_resources)?;
    f.write_all(&(layer_mask.len() as u32).to_be_bytes())?;
    f.write_all(&layer_mask)?;
    // Merged image data: one compression field for the whole section, then
    // the PackBits row-length table for every channel, then every row.
    // (Per-channel compression fields are only used in layer channel data.)
    f.write_all(&1u16.to_be_bytes())?;
    let mut row_lengths = Vec::with_capacity(3 * doc.height as usize * 2);
    let mut rows = Vec::new();
    for c in 0..3 {
        let channel = encode_channel(doc.width, doc.height, |x, y| {
            image[((y * doc.width + x) * 4 + c) as usize]
        })?;
        let table_end = 2 + doc.height as usize * 2;
        row_lengths.extend_from_slice(&channel[2..table_end]);
        rows.extend_from_slice(&channel[table_end..]);
    }
    f.write_all(&row_lengths)?;
    f.write_all(&rows)?;
    f.sync_all()?;
    drop(f);
    temp.persist(path)?;
    let mut warnings = Vec::new();
    if ps_layers
        .iter()
        .any(|ps| ps.layer.is_some_and(|l| l.linear_blend))
    {
        warnings.push("リニア合成設定はPSDに保存されません".to_string());
    }
    if doc
        .layers
        .iter()
        .any(|layer| layer.sketch || layer.reference)
    {
        warnings.push("下描き・参照レイヤー属性はPSDに保存されません".to_string());
    }
    if ps_layers
        .iter()
        .any(|ps| ps.layer.is_some_and(|l| l.locked))
    {
        warnings.push("レイヤーロック設定はPSDに保存されません".to_string());
    }
    if doc
        .layers
        .iter()
        .any(|layer| layer.kind == LayerKind::Folder && layer.blend != BlendMode::Normal)
    {
        warnings.push("フォルダーの合成モードはPSDに保存されません".to_string());
    }
    Ok(warnings)
}
pub struct PsdImport {
    pub document: Document,
    pub warnings: Vec<String>,
    pub warnings_en: Vec<String>,
}

pub fn import_psd(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
    Ok(import_psd_report(path)?.document)
}

pub fn import_psd_report(path: &Path) -> Result<PsdImport, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut f = File::open(path)?;
    let mut header = [0u8; 26];
    f.read_exact(&mut header)?;
    if &header[..4] != b"8BPS" || u16::from_be_bytes([header[4], header[5]]) != 1 {
        return Err("unsupported PSD header".into());
    }
    let channels = u16::from_be_bytes([header[12], header[13]]);
    let height = u32::from_be_bytes(header[14..18].try_into()?);
    let width = u32::from_be_bytes(header[18..22].try_into()?);
    let depth = u16::from_be_bytes([header[22], header[23]]);
    let mode = u16::from_be_bytes([header[24], header[25]]);
    let color_channels = match mode {
        1 => 1usize,
        3 => 3,
        4 => 4,
        _ => return Err("PSD color mode is not supported".into()),
    };
    if (channels as usize) < color_channels
        || channels > 56
        || !matches!(depth, 8 | 16)
        || width > MAX_DOCUMENT_DIMENSION
        || height > MAX_DOCUMENT_DIMENSION
    {
        return Err("PSD must be 8-bit or 16-bit grayscale, RGB, or CMYK".into());
    }
    let mut skip = [0u8; 4];
    f.read_exact(&mut skip)?;
    let color_mode_len = u32::from_be_bytes(skip) as u64;
    std::io::copy(
        &mut std::io::Read::by_ref(&mut f).take(color_mode_len),
        &mut std::io::sink(),
    )?;
    f.read_exact(&mut skip)?;
    let resources_len = u32::from_be_bytes(skip) as usize;
    if resources_len > 16 * 1024 * 1024 {
        return Err("PSD image resources exceed safety limit".into());
    }
    let mut resources = vec![0; resources_len];
    f.read_exact(&mut resources)?;
    let image_dpi = parse_psd_dpi(&resources).unwrap_or(300.0);
    f.read_exact(&mut skip)?;
    let layer_mask_len = u32::from_be_bytes(skip) as usize;
    if layer_mask_len > 512 * 1024 * 1024 {
        return Err("PSD layers exceed memory safety limit".into());
    }
    let mut layer_mask = vec![0; layer_mask_len];
    f.read_exact(&mut layer_mask)?;
    if width == 0 || height == 0 {
        return Err("PSD dimensions must be non-zero".into());
    }
    let mut compression = [0; 2];
    f.read_exact(&mut compression)?;
    let compression = u16::from_be_bytes(compression);
    if compression > 1 {
        return Err("unsupported PSD composite compression".into());
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or("PSD dimensions overflow")?;
    if n as u64 > MAX_DOCUMENT_PIXELS {
        return Err("PSD image exceeds safety limit".into());
    }
    let sample_bytes = (depth / 8) as usize;
    let estimated_bytes = n
        .checked_mul(4usize + channels as usize + sample_bytes)
        .ok_or("PSD allocation size overflow")?;
    if estimated_bytes > 512 * 1024 * 1024 {
        return Err("PSD image exceeds memory safety limit".into());
    }
    let mut planes = vec![vec![0; n]; channels as usize];
    if compression == 0 {
        for plane in &mut planes {
            let mut raw = vec![0; n * sample_bytes];
            f.read_exact(&mut raw)?;
            for (i, out) in plane.iter_mut().enumerate() {
                *out = if depth == 8 {
                    raw[i]
                } else {
                    let sample = u16::from_be_bytes([raw[i * 2], raw[i * 2 + 1]]) as u32;
                    ((sample * 255 + 32767) / 65535) as u8
                };
            }
        }
    } else {
        let row_count = channels as usize * height as usize;
        let mut row_lengths = vec![0u16; row_count];
        for length in &mut row_lengths {
            let mut bytes = [0; 2];
            f.read_exact(&mut bytes)?;
            *length = u16::from_be_bytes(bytes);
        }
        for channel in 0..channels as usize {
            for y in 0..height as usize {
                let length = row_lengths[channel * height as usize + y] as usize;
                let mut encoded = vec![0; length];
                f.read_exact(&mut encoded)?;
                let row = decode_packbits(&encoded, width as usize * sample_bytes)?;
                for x in 0..width as usize {
                    let value = if depth == 8 {
                        row[x]
                    } else {
                        let sample = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                        ((sample * 255 + 32767) / 65535) as u8
                    };
                    planes[channel][y * width as usize + x] = value;
                }
            }
        }
    }
    let mut doc = Document::new(width, height);
    doc.dpi = image_dpi;
    let alpha_channel = if channels as usize > color_channels {
        Some(color_channels)
    } else {
        None
    };
    for (i, first_channel) in planes[0].iter().enumerate() {
        let p = i * 4;
        match mode {
            1 => {
                let gray = *first_channel;
                doc.layers[0].pixels[p..p + 3].fill(gray);
            }
            3 => {
                for (c, plane) in planes.iter().take(3).enumerate() {
                    doc.layers[0].pixels[p + c] = plane[i];
                }
            }
            4 => {
                let (c, m, y, k) = (
                    planes[0][i] as u32,
                    planes[1][i] as u32,
                    planes[2][i] as u32,
                    planes[3][i] as u32,
                );
                // PSD stores inverted CMYK planes; alpha is not inverted.
                doc.layers[0].pixels[p] = ((c * k + 127) / 255) as u8;
                doc.layers[0].pixels[p + 1] = ((m * k + 127) / 255) as u8;
                doc.layers[0].pixels[p + 2] = ((y * k + 127) / 255) as u8;
            }
            _ => unreachable!(),
        }
        doc.layers[0].pixels[p + 3] = alpha_channel.map_or(255, |a| planes[a][i]);
    }
    let mut unsupported_reasons = Vec::new();
    let mut has_text_layers = false;
    let mut layers_preserved = false;
    if matches!(mode, 1 | 3 | 4) {
        let (layers, unsupported, text_layers, reasons) =
            parse_psd_layers(&layer_mask, width, height, mode, depth)?;
        unsupported_reasons = reasons;
        has_text_layers = text_layers;
        if !unsupported && let Some(layers) = layers {
            doc.layers = layers;
            layers_preserved = true;
        }
    }
    let mut warnings = Vec::new();
    let mut warnings_en = Vec::new();
    if depth != 8 {
        warnings.push("PSDの16bitレイヤーチャンネルを8bitへ変換しました".into());
        warnings_en.push("Converted 16-bit PSD layer channels to 8-bit.".into());
    }
    if mode == 4 && !layers_preserved && unsupported_reasons.is_empty() {
        warnings
            .push("レイヤー構造を読み取れないCMYK画像を8bit RGBの統合画像へ変換しました".into());
        warnings_en.push(
            "Could not preserve the CMYK layer structure; imported it as a merged 8-bit RGB image."
                .into(),
        );
    } else if !layers_preserved && unsupported_reasons.is_empty() {
        warnings.push("PSDに編集レイヤー情報がないため、統合画像として読み込みました".into());
        warnings_en
            .push("The PSD has no editable layer data; imported it as a merged image.".into());
    }
    if !unsupported_reasons.is_empty() {
        let reasons_ja = unsupported_reasons.join("、");
        let reasons_en = unsupported_reasons
            .iter()
            .map(|reason| match *reason {
                "合成モード" => "blend modes",
                "塗りつぶしレイヤー" => "fill layers",
                "スマートオブジェクト" => "smart objects",
                "レイヤー効果" => "layer effects",
                "ベクターマスク" => "vector masks",
                "調整レイヤー" => "adjustment layers",
                "圧縮方式" => "channel compression methods",
                "画素チャンネルのないレイヤー" => {
                    "layers without supported pixel channels"
                }
                _ => "unsupported layer features",
            })
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "未対応の{reasons_ja}があるため、PSDを統合画像として読み込みました"
        ));
        warnings_en.push(format!(
            "Unsupported {reasons_en} were found; imported the PSD as a merged image."
        ));
    }
    if has_text_layers && unsupported_reasons.is_empty() {
        warnings.push("PSDの文字レイヤーをラスター化して読み込みました".into());
        warnings_en.push("PSD text layers were imported as raster layers.".into());
    }
    Ok(PsdImport {
        document: doc,
        warnings,
        warnings_en,
    })
}

fn parse_psd_dpi(resources: &[u8]) -> Option<f32> {
    let mut pos = 0usize;
    while pos.checked_add(12)? <= resources.len() {
        if &resources[pos..pos + 4] != b"8BIM" {
            return None;
        }
        let id = u16::from_be_bytes([resources[pos + 4], resources[pos + 5]]);
        pos += 6;
        let name_len = *resources.get(pos)? as usize;
        let name_field_len = 1usize.checked_add(name_len)?;
        pos = pos.checked_add((name_field_len + 1) & !1)?;
        let data_len =
            u32::from_be_bytes(resources.get(pos..pos.checked_add(4)?)?.try_into().ok()?) as usize;
        pos += 4;
        let end = pos.checked_add(data_len)?;
        let data = resources.get(pos..end)?;
        if id == 0x03ED && data.len() >= 16 {
            let resolution = u32::from_be_bytes(data[..4].try_into().ok()?) as f32 / 65536.0;
            let unit = u16::from_be_bytes(data[4..6].try_into().ok()?);
            let dpi = match unit {
                1 => resolution,
                2 => resolution * 2.54,
                _ => return None,
            };
            return (dpi.is_finite() && dpi > 0.0).then_some(dpi);
        }
        pos = end.checked_add(data_len & 1)?;
    }
    None
}

fn be_u16<R: std::io::Read>(r: &mut R) -> Result<u16, Box<dyn std::error::Error>> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}
fn be_u32<R: std::io::Read>(r: &mut R) -> Result<u32, Box<dyn std::error::Error>> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}
fn be_i32<R: std::io::Read>(r: &mut R) -> Result<i32, Box<dyn std::error::Error>> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
fn psd_rect_tile_count(bounds: (i32, i32, i32, i32), width: u32, height: u32) -> u64 {
    let top = i64::from(bounds.0).clamp(0, i64::from(height)) as u32;
    let left = i64::from(bounds.1).clamp(0, i64::from(width)) as u32;
    let bottom = i64::from(bounds.2).clamp(0, i64::from(height)) as u32;
    let right = i64::from(bounds.3).clamp(0, i64::from(width)) as u32;
    if top >= bottom || left >= right {
        return 0;
    }
    u64::from((right - 1) / TILE_SIZE - left / TILE_SIZE + 1)
        * u64::from((bottom - 1) / TILE_SIZE - top / TILE_SIZE + 1)
}

#[allow(clippy::type_complexity)] // Compact private parse result for feature support flags and layers.
fn parse_psd_layers(
    section: &[u8],
    width: u32,
    height: u32,
    color_mode: u16,
    depth: u16,
) -> Result<(Option<Vec<Layer>>, bool, bool, Vec<&'static str>), Box<dyn std::error::Error>> {
    parse_psd_layers_with_tile_limit(
        section,
        width,
        height,
        color_mode,
        depth,
        MAX_PSD_TILE_MEMORY,
    )
}

#[allow(clippy::type_complexity)] // Same parse result as the production wrapper.
fn parse_psd_layers_with_tile_limit(
    section: &[u8],
    width: u32,
    height: u32,
    color_mode: u16,
    depth: u16,
    tile_memory_limit: u64,
) -> Result<(Option<Vec<Layer>>, bool, bool, Vec<&'static str>), Box<dyn std::error::Error>> {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    if section.len() < 4 {
        return Ok((None, false, false, Vec::new()));
    }
    let mut outer = Cursor::new(section);
    let info_len = be_u32(&mut outer)? as usize;
    if info_len == 0 {
        return Ok((None, false, false, Vec::new()));
    }
    if info_len > section.len().saturating_sub(4) {
        return Err("invalid PSD layer info length".into());
    }
    let mut info = vec![0; info_len];
    outer.read_exact(&mut info)?;
    let mut r = Cursor::new(info);
    let count = (be_u16(&mut r)? as i16).unsigned_abs() as usize;
    if count > 400 {
        return Err("PSD layer count exceeds safety limit".into());
    }
    struct Channel {
        id: i16,
        len: usize,
    }
    struct Record {
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
        channels: Vec<Channel>,
        name: String,
        opacity: u8,
        visible: bool,
        clipping: bool,
        blend: BlendMode,
        has_mask: bool,
        mask_bounds: Option<(i32, i32, i32, i32)>,
        mask_default: u8,
        section: Option<u32>,
    }
    let mut records = Vec::with_capacity(count);
    let mut has_unsupported_features = false;
    let mut has_text_layers = false;
    let mut unsupported_reasons = Vec::new();
    for _ in 0..count {
        let top = be_i32(&mut r)?;
        let left = be_i32(&mut r)?;
        let bottom = be_i32(&mut r)?;
        let right = be_i32(&mut r)?;
        let n = be_u16(&mut r)? as usize;
        if n > 64 {
            return Err("PSD layer channel count exceeds safety limit".into());
        }
        let mut channels = Vec::with_capacity(n);
        for _ in 0..n {
            channels.push(Channel {
                id: be_u16(&mut r)? as i16,
                len: be_u32(&mut r)? as usize,
            });
        }
        let mut fixed = [0; 12];
        r.read_exact(&mut fixed)?;
        if &fixed[..4] != b"8BIM" {
            return Err("invalid PSD layer blend signature".into());
        }
        let blend = match &fixed[4..8] {
            b"norm" | b"pass" => BlendMode::Normal,
            b"mul " => BlendMode::Multiply,
            b"scrn" => BlendMode::Screen,
            b"over" => BlendMode::Overlay,
            b"dark" => BlendMode::Darken,
            b"lite" => BlendMode::Lighten,
            b"div " => BlendMode::ColorDodge,
            b"idiv" => BlendMode::ColorBurn,
            b"hLit" => BlendMode::HardLight,
            b"sLit" => BlendMode::SoftLight,
            b"diff" => BlendMode::Difference,
            b"smud" => BlendMode::Exclusion,
            b"lddg" => BlendMode::Add,
            b"fsub" => BlendMode::Subtract,
            _ => {
                has_unsupported_features = true;
                if !unsupported_reasons.contains(&"合成モード") {
                    unsupported_reasons.push("合成モード");
                }
                BlendMode::Normal
            }
        };
        let opacity = fixed[8];
        let clipping = fixed[9] != 0;
        let visible = fixed[10] & 2 == 0;
        let extra_len = be_u32(&mut r)? as usize;
        let start = r.position() as usize;
        let end = start
            .checked_add(extra_len)
            .ok_or("PSD extra data overflow")?;
        if extra_len < 12 || end > r.get_ref().len() {
            return Err("invalid PSD layer extra data".into());
        }
        let mask_len = be_u32(&mut r)? as usize;
        if (r.position() as usize)
            .checked_add(mask_len)
            .is_none_or(|mask_end| mask_end > end.saturating_sub(8))
        {
            return Err("PSD layer mask data exceeds its extra-data section".into());
        }
        let mut has_mask = mask_len >= 18;
        let mut mask_bounds = None;
        let mut mask_default = 255;
        if has_mask {
            let mut mask_header = [0u8; 18];
            r.read_exact(&mut mask_header)?;
            let mut bounds = [
                i32::from_be_bytes(mask_header[0..4].try_into()?),
                i32::from_be_bytes(mask_header[4..8].try_into()?),
                i32::from_be_bytes(mask_header[8..12].try_into()?),
                i32::from_be_bytes(mask_header[12..16].try_into()?),
            ];
            let flags = mask_header[17];
            // Disabled masks still have channel data to consume, but do not
            // affect the imported layer's appearance.
            has_mask = flags & 2 == 0;
            if flags & 1 != 0 {
                for (bound, offset) in bounds.iter_mut().zip([top, left, top, left]) {
                    *bound = bound
                        .checked_add(offset)
                        .ok_or("PSD layer-relative mask coordinate overflow")?;
                }
            }
            mask_bounds = Some((bounds[0], bounds[1], bounds[2], bounds[3]));
            mask_default = mask_header[16];
            r.seek(SeekFrom::Current(mask_len.saturating_sub(18) as i64))?;
        } else {
            r.seek(SeekFrom::Current(mask_len as i64))?;
        }
        let blend_len = be_u32(&mut r)? as usize;
        if (r.position() as usize)
            .checked_add(blend_len)
            .is_none_or(|blend_end| blend_end > end.saturating_sub(4))
        {
            return Err("PSD blending ranges exceed their extra-data section".into());
        }
        r.seek(SeekFrom::Current(blend_len as i64))?;
        let mut name_len = [0u8; 1];
        r.read_exact(&mut name_len)?;
        let name_len = name_len[0] as usize;
        let pad = (4 - ((name_len + 1) % 4)) % 4;
        if (r.position() as usize)
            .checked_add(name_len + pad)
            .is_none_or(|name_end| name_end > end)
        {
            return Err("PSD layer name exceeds its extra-data section".into());
        }
        let mut name_bytes = vec![0; name_len];
        r.read_exact(&mut name_bytes)?;
        let mut name = String::from_utf8_lossy(&name_bytes).into_owned();
        r.seek(SeekFrom::Current(pad as i64))?;
        let mut section = None;
        while (r.position() as usize).saturating_add(12) <= end {
            let mut sig = [0; 4];
            r.read_exact(&mut sig)?;
            let mut key = [0; 4];
            r.read_exact(&mut key)?;
            let len = be_u32(&mut r)? as usize;
            if &sig != b"8BIM" {
                break;
            }
            if (r.position() as usize)
                .checked_add(len)
                .is_none_or(|block_end| block_end > end)
            {
                return Err("PSD tagged layer data exceeds its extra-data section".into());
            }
            if &key == b"TySh" {
                has_text_layers = true;
            }
            if matches!(
                &key,
                b"SoCo"
                    | b"GdFl"
                    | b"PtFl"
                    | b"PlLd"
                    | b"lnk2"
                    | b"lfx2"
                    | b"lrFX"
                    | b"vmsk"
                    | b"vsms"
                    | b"levl"
                    | b"curv"
                    | b"brit"
                    | b"hue2"
                    | b"selc"
                    | b"mixr"
                    | b"clrL"
                    | b"expA"
                    | b"blwh"
                    | b"phfl"
                    | b"vibA"
                    | b"grdm"
                    | b"nvrt"
                    | b"thrs"
                    | b"post"
            ) {
                has_unsupported_features = true;
                let reason = match &key {
                    b"SoCo" | b"GdFl" | b"PtFl" => "塗りつぶしレイヤー",
                    b"PlLd" | b"lnk2" => "スマートオブジェクト",
                    b"lfx2" | b"lrFX" => "レイヤー効果",
                    b"vmsk" | b"vsms" => "ベクターマスク",
                    _ => "調整レイヤー",
                };
                if !unsupported_reasons.contains(&reason) {
                    unsupported_reasons.push(reason);
                }
            }
            if &key == b"lsct" && len >= 4 {
                section = Some(be_u32(&mut r)?);
                r.seek(SeekFrom::Current(len.saturating_sub(4) as i64))?;
            } else if &key == b"luni" && len >= 4 {
                // Unicode layer name: u32 length, then UTF-16BE units.
                let count = be_u32(&mut r)? as usize;
                if count > (len - 4) / 2 {
                    return Err("PSD Unicode layer name exceeds its tagged block".into());
                }
                let units = count;
                let mut utf16 = Vec::with_capacity(units);
                for _ in 0..units {
                    utf16.push(be_u16(&mut r)?);
                }
                while utf16.last() == Some(&0) {
                    utf16.pop();
                }
                name = String::from_utf16_lossy(&utf16);
                r.seek(SeekFrom::Current((len - 4 - units * 2) as i64))?;
            } else {
                r.seek(SeekFrom::Current(len as i64))?;
            }
        }
        r.set_position(end as u64);
        let required_color_channels: &[i16] = match color_mode {
            1 => &[0],
            4 => &[0, 1, 2, 3],
            _ => &[0, 1, 2],
        };
        if !matches!(section, Some(1..=3))
            && required_color_channels
                .iter()
                .any(|channel_id| !channels.iter().any(|channel| channel.id == *channel_id))
        {
            // Adjustment and fill layers commonly have no RGB pixel planes.
            // Keep the reliable merged composite instead of importing a blank layer.
            has_unsupported_features = true;
            if !unsupported_reasons.contains(&"画素チャンネルのないレイヤー") {
                unsupported_reasons.push("画素チャンネルのないレイヤー");
            }
        }
        records.push(Record {
            top,
            left,
            bottom,
            right,
            channels,
            name,
            opacity,
            visible,
            clipping,
            blend,
            has_mask,
            mask_bounds,
            mask_default,
            section,
        });
    }
    let mut layers: Vec<Layer> = Vec::with_capacity(count);
    let mut group_stack: Vec<u64> = Vec::new();
    let mut decoded_channel_bytes = 0u64;
    let mut decoded_tile_bytes = 0u64;
    let mut cmyk_scratch_bytes = 0usize;
    for record in records {
        if record.section == Some(3) {
            // Section dividers can have empty channel payloads too. They do not
            // become layers, but their bytes must not be read as the next layer.
            for channel in record.channels {
                let end = (r.position() as usize)
                    .checked_add(channel.len)
                    .ok_or("PSD section-divider channel size overflow")?;
                if channel.len < 2 || end > r.get_ref().len() {
                    return Err("invalid PSD section-divider channel data".into());
                }
                r.set_position(end as u64);
            }
            group_stack.pop();
            continue;
        }
        let layer_rect = (record.top, record.left, record.bottom, record.right);
        let mask_rect = record.mask_bounds.unwrap_or(layer_rect);
        let pixel_tiles = psd_rect_tile_count(layer_rect, width, height);
        let mask_tiles = if record.has_mask {
            let changed_tiles = if record.channels.iter().any(|channel| channel.id == -2) {
                psd_rect_tile_count(mask_rect, width, height)
            } else {
                0
            };
            // Nonwhite defaults share one buffer. Count possible COW tiles too;
            // this conservatively keeps the default even if every tile changes.
            changed_tiles + u64::from(record.mask_default != 255)
        } else {
            0
        };
        let tile_bytes = (pixel_tiles + mask_tiles)
            .checked_mul(u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4)
            .ok_or("PSD decoded tile buffer size overflow")?;
        decoded_tile_bytes = decoded_tile_bytes
            .checked_add(tile_bytes)
            .ok_or("PSD decoded tile buffer size overflow")?;
        if decoded_tile_bytes > tile_memory_limit {
            return Err("PSD decoded tile buffer estimate exceeds the memory safety limit".into());
        }
        let id = layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let mut layer = Layer::new(id, record.name, width, height);
        layer.parent_id = group_stack.last().copied();
        if matches!(record.section, Some(1 | 2)) {
            layer.kind = LayerKind::Folder;
            layer.expanded = record.section == Some(1);
        }
        layer.opacity = record.opacity as f32 / 255.;
        layer.visible = record.visible;
        layer.clipping = record.clipping;
        layer.blend = record.blend;
        if record.has_mask {
            layer.mask = Some(efude_canvas::TilePixels::new(width, height));
            if record.mask_default != 255 {
                layer
                    .mask
                    .as_mut()
                    .unwrap()
                    .fill_shared([record.mask_default; 4]);
            }
        }
        let lw = (record.right as i64 - record.left as i64).max(0) as usize;
        let lh = (record.bottom as i64 - record.top as i64).max(0) as usize;
        if lw > width as usize * 2
            || lh > height as usize * 2
            || lw.saturating_mul(lh) > 100_000_000
        {
            return Err("PSD layer dimensions exceed safety limit".into());
        }
        let mut cmyk_black = if color_mode == 4 && layer.kind == LayerKind::Raster {
            let scratch_len = lw.checked_mul(lh).ok_or("PSD CMYK layer size overflow")?;
            cmyk_scratch_bytes = cmyk_scratch_bytes
                .checked_add(scratch_len)
                .ok_or("PSD CMYK scratch size overflow")?;
            if cmyk_scratch_bytes > 512 * 1024 * 1024 {
                return Err("PSD CMYK layers exceed memory safety limit".into());
            }
            Some(vec![0; scratch_len])
        } else {
            None
        };
        let mask_width = (mask_rect.3 as i64 - mask_rect.1 as i64).max(0) as usize;
        let mask_height = (mask_rect.2 as i64 - mask_rect.0 as i64).max(0) as usize;
        if mask_width > width as usize * 2
            || mask_height > height as usize * 2
            || mask_width.saturating_mul(mask_height) > 100_000_000
        {
            return Err("PSD layer mask dimensions exceed safety limit".into());
        }
        let has_alpha = record.channels.iter().any(|c| c.id == -1);
        if !has_alpha {
            for y in 0..lh {
                let dy = record.top + y as i32;
                if dy < 0 || dy >= height as i32 {
                    continue;
                }
                for x in 0..lw {
                    let dx = record.left + x as i32;
                    if dx >= 0 && dx < width as i32 {
                        let i = ((dy as u32 * width + dx as u32) * 4 + 3) as usize;
                        layer.pixels[i] = 255;
                    }
                }
            }
        }
        for channel in record.channels {
            let (channel_left, channel_top, channel_width, channel_height) = if channel.id == -2 {
                (mask_rect.1, mask_rect.0, mask_width, mask_height)
            } else {
                (record.left, record.top, lw, lh)
            };
            let decoded_len = channel_width
                .checked_mul(channel_height)
                .ok_or("PSD channel dimensions overflow")?;
            let sample_bytes = (depth / 8) as usize;
            let decoded_bytes = decoded_len
                .checked_mul(sample_bytes)
                .ok_or("PSD channel byte size overflow")?;
            let row_bytes = channel_width
                .checked_mul(sample_bytes)
                .ok_or("PSD channel row size overflow")?;
            let encoded_limit = decoded_bytes
                .checked_mul(2)
                .and_then(|n| n.checked_add(channel_height.saturating_mul(2)))
                .and_then(|n| n.checked_add(2))
                .ok_or("PSD channel size overflow")?;
            if channel.len > encoded_limit {
                return Err("PSD channel is larger than its layer dimensions allow".into());
            }
            decoded_channel_bytes = decoded_channel_bytes
                .checked_add(decoded_bytes as u64)
                .ok_or("PSD decoded channel size overflow")?;
            if decoded_channel_bytes > 512 * 1024 * 1024 {
                return Err("PSD decoded layers exceed the 512 MiB safety limit".into());
            }
            if channel.len < 2 {
                return Err("PSD layer channel is missing its compression field".into());
            }
            if (r.position() as usize)
                .checked_add(channel.len)
                .is_none_or(|channel_end| channel_end > r.get_ref().len())
            {
                return Err("PSD layer channel exceeds its layer-info section".into());
            }
            let compression = be_u16(&mut r)?;
            let mut copy_row = |y: usize, row: &[u8]| {
                let dy = channel_top as i64 + y as i64;
                if dy < 0 || dy >= height as i64 {
                    return;
                }
                for x in 0..channel_width {
                    let dx = channel_left as i64 + x as i64;
                    if dx < 0 || dx >= width as i64 {
                        continue;
                    }
                    let dst = ((dy as u32 * width + dx as u32) * 4) as usize;
                    let value = if sample_bytes == 1 {
                        row[x]
                    } else {
                        let sample = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                        ((sample * 255 + 32767) / 65535) as u8
                    };
                    match channel.id {
                        0 => {
                            layer.pixels[dst] = value;
                            if color_mode == 1 {
                                layer.pixels[dst + 1] = value;
                                layer.pixels[dst + 2] = value;
                            }
                        }
                        1 => layer.pixels[dst + 1] = value,
                        2 => layer.pixels[dst + 2] = value,
                        3 if color_mode == 4 => {
                            if let Some(black) = &mut cmyk_black {
                                black[y * channel_width + x] = value;
                            }
                        }
                        -1 => layer.pixels[dst + 3] = value,
                        -2 => {
                            if let Some(mask) = &mut layer.mask {
                                // Pixels outside the mask rectangle keep its
                                // default, including the rest of this tile.
                                if !mask.has_tile(dx as u32, dy as u32) {
                                    mask.ensure_tile_filled(
                                        dx as u32,
                                        dy as u32,
                                        [record.mask_default; 4],
                                    );
                                }
                                mask.set_pixel(dx as u32, dy as u32, [value; 4]);
                            }
                        }
                        _ => {}
                    }
                }
            };
            match compression {
                0 => {
                    if channel.len < 2 + decoded_bytes {
                        return Err("PSD raw layer channel is shorter than its dimensions".into());
                    }
                    let mut row = vec![0; row_bytes];
                    for y in 0..channel_height {
                        r.read_exact(&mut row)?;
                        copy_row(y, &row);
                    }
                    r.seek(SeekFrom::Current((channel.len - 2 - decoded_bytes) as i64))?;
                }
                1 => {
                    let table_bytes = channel_height
                        .checked_mul(2)
                        .ok_or("PSD row table size overflow")?;
                    if channel.len < 2 + table_bytes {
                        return Err("PSD PackBits row table is truncated".into());
                    }
                    let mut lengths = vec![0u16; channel_height];
                    for length in &mut lengths {
                        *length = be_u16(&mut r)?;
                    }
                    let mut consumed = 2 + table_bytes;
                    for (y, length) in lengths.into_iter().enumerate() {
                        let length = length as usize;
                        consumed = consumed
                            .checked_add(length)
                            .ok_or("PSD PackBits channel size overflow")?;
                        if consumed > channel.len {
                            return Err("PSD PackBits rows exceed their channel size".into());
                        }
                        let mut encoded = vec![0; length];
                        r.read_exact(&mut encoded)?;
                        let row = decode_packbits(&encoded, row_bytes)?;
                        copy_row(y, &row);
                    }
                    r.seek(SeekFrom::Current((channel.len - consumed) as i64))?;
                }
                _ => {
                    has_unsupported_features = true;
                    if !unsupported_reasons.contains(&"圧縮方式") {
                        unsupported_reasons.push("圧縮方式");
                    }
                    r.seek(SeekFrom::Current((channel.len - 2) as i64))?;
                }
            }
        }
        if let Some(black) = cmyk_black {
            for y in 0..lh {
                let dy = record.top as i64 + y as i64;
                if dy < 0 || dy >= height as i64 {
                    continue;
                }
                for x in 0..lw {
                    let dx = record.left as i64 + x as i64;
                    if dx < 0 || dx >= width as i64 {
                        continue;
                    }
                    let index = ((dy as u32 * width + dx as u32) * 4) as usize;
                    let (c, m, yellow, k) = (
                        layer.pixels[index] as u32,
                        layer.pixels[index + 1] as u32,
                        layer.pixels[index + 2] as u32,
                        black[y * lw + x] as u32,
                    );
                    layer.pixels[index] = ((c * k + 127) / 255) as u8;
                    layer.pixels[index + 1] = ((m * k + 127) / 255) as u8;
                    layer.pixels[index + 2] = ((yellow * k + 127) / 255) as u8;
                }
            }
        }
        if layer.kind == LayerKind::Folder {
            group_stack.push(layer.id);
        }
        layers.push(layer);
    }
    if layers.len() > 200 {
        return Err("PSD contains more than 200 editable layers".into());
    }
    if layers.is_empty() {
        Ok((
            None,
            has_unsupported_features,
            has_text_layers,
            unsupported_reasons,
        ))
    } else {
        layers.reverse();
        Ok((
            Some(layers),
            has_unsupported_features,
            has_text_layers,
            unsupported_reasons,
        ))
    }
}
fn decode_packbits(input: &[u8], expected: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while i < input.len() && out.len() < expected {
        let n = input[i] as i8;
        i += 1;
        match n {
            0..=127 => {
                let count = n as usize + 1;
                if i + count > input.len() {
                    return Err("invalid PSD PackBits literal run".into());
                }
                out.extend_from_slice(&input[i..i + count]);
                i += count;
            }
            -127..=-1 => {
                let count = 1 + (-(n as i16)) as usize;
                let value = *input.get(i).ok_or("invalid PSD PackBits repeat run")?;
                i += 1;
                out.extend(std::iter::repeat_n(value, count));
            }
            -128 => {}
        }
    }
    if out.len() != expected {
        return Err("PSD PackBits row has an unexpected length".into());
    }
    Ok(out)
}
pub fn save_backup(
    path: &Path,
    doc: &Document,
    generations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let dir = parent.join(".efude-backups");
    fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let prefix = format!("{stem}.");
    let mut entries = Vec::new();
    for item in fs::read_dir(&dir)? {
        let entry = item?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(timestamp) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|name| name.strip_suffix(".efude"))
        else {
            continue;
        };
        // Only this document's numeric timestamp may follow its exact stem.
        // For example, "chapter.rough.*.efude" belongs to another document.
        if timestamp.is_empty() || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if let Ok(timestamp) = timestamp.parse::<u128>() {
            entries.push((timestamp, entry));
        }
    }
    let stamp = next_backup_stamp(stamp, entries.iter().map(|(stamp, _)| *stamp).max())?;
    let backup = dir.join(format!("{stem}.{stamp}.efude"));
    save(&backup, doc)?;
    entries.sort_by_key(|(timestamp, _)| *timestamp);
    // The pre-save list contains only previous generations. The newly saved
    // generation sorts last even after a wall-clock correction.
    let excess = entries
        .len()
        .saturating_add(1)
        .saturating_sub(generations.max(1));
    for (_, entry) in entries.into_iter().take(excess) {
        let _ = fs::remove_file(entry.path());
    }
    Ok(())
}
fn next_backup_stamp(
    now: u128,
    previous: Option<u128>,
) -> Result<u128, Box<dyn std::error::Error>> {
    match previous {
        Some(previous) => Ok(now.max(previous.checked_add(1).ok_or("backup timestamp overflow")?)),
        None => Ok(now),
    }
}
pub fn backup_interval(minutes: u32) -> Duration {
    Duration::from_secs(minutes.clamp(1, 120) as u64 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_vector_document() -> Document {
        use efude_canvas::{VectorPoint, VectorStroke};
        let mut document = Document::new(8, 8);
        document.layers[0].vector = Some(vec![VectorStroke::fitted(
            vec![
                VectorPoint {
                    x: 1.0,
                    y: 2.0,
                    width: 2.0,
                },
                VectorPoint {
                    x: 6.0,
                    y: 4.0,
                    width: 1.0,
                },
            ],
            [20, 40, 80, 255],
            0.8,
        )]);
        document.layers[0].pixels.set_pixel(1, 2, [20, 40, 80, 255]);
        document
    }

    fn sample_document() -> Document {
        let mut document = Document::new(4, 3);
        document.layers[0]
            .pixels
            .set_pixel(1, 1, [220, 48, 76, 255]);
        document.layers[0]
            .pixels
            .set_pixel(2, 1, [18, 90, 240, 127]);
        document
    }

    #[test]
    fn thumbnail_loading_enforces_preview_dimensions() {
        let directory = tempfile::tempdir().unwrap();
        for (width, height) in [(256, 256), (257, 1), (1, 257)] {
            let path = directory.path().join(format!("{width}-{height}.efude"));
            let mut png_bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[20, 40, 80, 255].repeat((width * height) as usize))
                .unwrap();
            let mut archive = ZipWriter::new(File::create(&path).unwrap());
            archive
                .start_file("thumbnail.png", SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&png_bytes).unwrap();
            archive.finish().unwrap();
            let result = load_thumbnail(&path);
            if width <= 256 && height <= 256 {
                let (loaded_width, loaded_height, pixels) = result.unwrap();
                assert_eq!((loaded_width, loaded_height), (width, height));
                assert_eq!(pixels.len(), (width * height * 4) as usize);
            } else {
                assert!(
                    result.is_err(),
                    "oversized thumbnail accepted: {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn backup_retention_keeps_documents_with_overlapping_stems_separate() {
        for other_stem in ["chapter.rough", "chapter.1"] {
            let directory = tempfile::tempdir().unwrap();
            let mut other = sample_document();
            other.layers[0].pixels.set_pixel(0, 0, [7, 8, 9, 255]);
            save_backup(
                &directory.path().join(format!("{other_stem}.efude")),
                &other,
                1,
            )
            .unwrap();
            let backup_dir = directory.path().join(".efude-backups");
            let other_backup = fs::read_dir(&backup_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();

            let current = sample_document();
            save_backup(&directory.path().join("chapter.efude"), &current, 1).unwrap();
            let backups = fs::read_dir(&backup_dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(backups.len(), 2, "overlapping stem: {other_stem}");
            assert!(
                other_backup.exists(),
                "the other document's backup was deleted"
            );
            assert_eq!(
                load(&other_backup).unwrap().layers[0].pixels.pixel(0, 0),
                [7, 8, 9, 255]
            );
            let current_backup = backups.iter().find(|path| **path != other_backup).unwrap();
            assert_eq!(
                load(current_backup).unwrap().layers[0].pixels.to_dense(),
                current.layers[0].pixels.to_dense()
            );
        }
    }

    #[test]
    fn backup_retention_keeps_current_generation_when_clock_moves_backwards() {
        for generations in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let backup_dir = directory.path().join(".efude-backups");
            fs::create_dir(&backup_dir).unwrap();
            // These older saves model a wall clock that was one hour ahead.
            let future_stamp = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
                + Duration::from_secs(3600))
            .as_nanos();
            let oldest = backup_dir.join(format!("chapter.{future_stamp}.efude"));
            let newest = backup_dir.join(format!("chapter.{}.efude", future_stamp + 1));
            let mut document = sample_document();
            save(&oldest, &document).unwrap();
            save(&newest, &document).unwrap();

            // Consecutive saves must both retain their current contents, even
            // while all previous filenames still sort after the current time.
            let mut previous_color = None;
            for color in [[11, 37, 83, 255], [19, 43, 97, 255]] {
                document.layers[0].pixels.set_pixel(1, 1, color);
                save_backup(
                    &directory.path().join("chapter.efude"),
                    &document,
                    generations,
                )
                .unwrap();
                let backups = fs::read_dir(&backup_dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect::<Vec<_>>();
                assert_eq!(backups.len(), generations);
                assert!(
                    backups
                        .iter()
                        .any(|path| load(path).unwrap().layers[0].pixels.pixel(1, 1) == color),
                    "the current backup was deleted for {generations} retained generations"
                );
                if let Some(previous_color) = previous_color {
                    assert_eq!(
                        backups.iter().any(|path| {
                            load(path).unwrap().layers[0].pixels.pixel(1, 1) == previous_color
                        }),
                        generations == 2,
                        "the previous save must be the second newest generation"
                    );
                }
                previous_color = Some(color);
            }
            assert!(!oldest.exists());
            assert!(!newest.exists());
        }
    }

    #[test]
    fn backup_stamp_preserves_order_and_rejects_overflow_without_writes() {
        assert_eq!(next_backup_stamp(100, None).unwrap(), 100);
        assert_eq!(next_backup_stamp(100, Some(99)).unwrap(), 100);
        assert_eq!(next_backup_stamp(100, Some(100)).unwrap(), 101);
        assert_eq!(next_backup_stamp(100, Some(101)).unwrap(), 102);
        assert_eq!(next_backup_stamp(u128::MAX, None).unwrap(), u128::MAX);
        assert_eq!(
            next_backup_stamp(100, Some(u128::MAX - 1)).unwrap(),
            u128::MAX
        );

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chapter.efude");
        let backup_dir = directory.path().join(".efude-backups");
        fs::create_dir(&backup_dir).unwrap();
        let document = sample_document();
        save(&path, &document).unwrap();
        let old_backup = backup_dir.join("chapter.100.efude");
        let max_backup = backup_dir.join(format!("chapter.{}.efude", u128::MAX));
        save(&old_backup, &document).unwrap();
        save(&max_backup, &document).unwrap();
        let original = fs::read(&path).unwrap();
        let old_bytes = fs::read(&old_backup).unwrap();
        let max_bytes = fs::read(&max_backup).unwrap();
        let mut changed = document;
        changed.layers[0].pixels.set_pixel(1, 1, [11, 37, 83, 255]);

        let error = save_backup(&path, &changed, 1).unwrap_err().to_string();
        assert_eq!(error, "backup timestamp overflow");
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&old_backup).unwrap(), old_bytes);
        assert_eq!(fs::read(&max_backup).unwrap(), max_bytes);
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 2);
    }

    #[test]
    fn backup_stamp_ignores_other_documents_and_non_file_entries() {
        let directory = tempfile::tempdir().unwrap();
        let backup_dir = directory.path().join(".efude-backups");
        fs::create_dir(&backup_dir).unwrap();
        let ignored_folder = backup_dir.join(format!("chapter.{}.efude", u128::MAX));
        fs::create_dir(&ignored_folder).unwrap();
        let document = sample_document();
        let other_document = backup_dir.join(format!("chapter.rough.{}.efude", u128::MAX));
        save(&other_document, &document).unwrap();
        let other_bytes = fs::read(&other_document).unwrap();

        save_backup(&directory.path().join("chapter.efude"), &document, 1).unwrap();

        assert!(ignored_folder.is_dir());
        assert_eq!(fs::read(&other_document).unwrap(), other_bytes);
        let backups = fs::read_dir(&backup_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file() && path != &other_document)
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            load(&backups[0]).unwrap().layers[0].pixels.pixel(1, 1),
            document.layers[0].pixels.pixel(1, 1)
        );
    }

    #[test]
    fn backup_retention_prunes_only_timestamped_generations() {
        let directory = tempfile::tempdir().unwrap();
        let backup_dir = directory.path().join(".efude-backups");
        fs::create_dir(&backup_dir).unwrap();
        let document = sample_document();
        let oldest = backup_dir.join("chapter.100.efude");
        let newer = backup_dir.join("chapter.101.efude");
        save(&oldest, &document).unwrap();
        save(&newer, &document).unwrap();
        let unrelated = [
            "chapter..efude",
            "chapter.notes.efude",
            "chapter.1.102.efude",
        ];
        for name in unrelated {
            save(&backup_dir.join(name), &document).unwrap();
        }

        save_backup(&directory.path().join("chapter.efude"), &document, 2).unwrap();
        assert!(!oldest.exists());
        assert!(newer.exists());
        for name in unrelated {
            assert!(
                backup_dir.join(name).exists(),
                "unrelated file deleted: {name}"
            );
        }
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 5);
    }

    #[test]
    fn save_and_image_exports_accept_filename_only_paths() {
        let document = sample_document();
        let mut failures = Vec::new();
        for extension in ["efude", "png", "jpg"] {
            let output = tempfile::Builder::new()
                .suffix(&format!(".{extension}"))
                .tempfile_in(".")
                .unwrap()
                .into_temp_path();
            let relative = Path::new(output.file_name().unwrap());
            let result = match extension {
                "efude" => save(relative, &document),
                "png" => export_png(relative, &document),
                "jpg" => export_jpeg(relative, &document, 90),
                _ => unreachable!(),
            };
            match result {
                Ok(()) => assert!(fs::metadata(relative).unwrap().len() > 0),
                Err(error) => failures.push(format!("{extension}: {error}")),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    #[test]
    fn efude_rejects_unreadable_optional_entries_instead_of_losing_data() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = sample_document();
        document.layers[0].vector = Some(Vec::new());
        let mut mask = efude_canvas::TilePixels::new(4, 3);
        mask.ensure_tile_filled(0, 0, [255; 4]);
        document.layers[0].mask = Some(mask);
        document
            .metadata
            .insert("comic".into(), "important settings".into());
        for entry_name in [
            "tiles/1/0_0.bin",
            "vectors/1.json",
            "masks/1/0_0.gray",
            "metadata.json",
        ] {
            let path = directory.path().join("unreadable.efude");
            save(&path, &document).unwrap();
            let (local, central) = {
                let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
                let entry = archive.by_name(entry_name).unwrap();
                (
                    entry.header_start() as usize,
                    entry.central_header_start() as usize,
                )
            };
            let mut bytes = fs::read(&path).unwrap();
            // Mark the existing entry as encrypted. It exists, but cannot be
            // opened without a password and must never be treated as absent.
            bytes[local + 6] |= 1;
            bytes[central + 8] |= 1;
            fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err(), "silently discarded {entry_name}");
        }
    }

    #[test]
    fn efude_rejects_nonfinite_vector_control_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-vector.efude");
        for field in [
            "x", "y", "width", "in_x", "in_y", "out_x", "out_y", "hardness",
        ] {
            let mut stroke = serde_json::json!({
                "points": [{"x": 1.0, "y": 1.0, "width": 2.0}],
                "color": [20, 40, 80, 255],
                "hardness": 1.0,
                "anchors": [{
                    "x": 1.0, "y": 1.0, "width": 2.0,
                    "in_x": 0.0, "in_y": 0.0, "out_x": 0.0, "out_y": 0.0
                }]
            });
            // A finite JSON f64 can overflow when decoded into an f32.
            if field == "hardness" {
                stroke[field] = serde_json::json!(3.5e38);
            } else {
                stroke["anchors"][0][field] = serde_json::json!(3.5e38);
            }
            let vectors = serde_json::to_vec(&vec![stroke]).unwrap();
            let mut archive = ZipWriter::new(File::create(&path).unwrap());
            for (name, bytes) in [
                ("mimetype", b"application/x-efude".as_slice()),
                (
                    "manifest.json",
                    br#"{"format_version":"0.1","width":4,"height":3,"color_space":"sRGB","pixel_format":"RGBA8"}"#.as_slice(),
                ),
                ("layers.json", br#"[{"id":1}]"#.as_slice()),
                ("vectors/1.json", vectors.as_slice()),
            ] {
                archive
                    .start_file(
                        name,
                        SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Stored),
                    )
                    .unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
            assert!(load(&path).is_err(), "invalid vector {field} accepted");
        }
    }

    #[test]
    fn efude_save_rejects_nonfinite_vectors_without_replacing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keep-valid-vector.efude");
        let original = small_vector_document();
        save(&path, &original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        assert_eq!(
            load(&path).unwrap().layers[0].vector,
            original.layers[0].vector
        );
        let mut accepted = Vec::new();
        let mut replaced = 0;
        let mut unreadable = 0;
        for field in 0..11 {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                // Each case attempts to replace the same valid native file.
                fs::write(&path, &original_bytes).unwrap();
                let mut invalid = original.clone();
                let stroke = &mut invalid.layers[0].vector.as_mut().unwrap()[0];
                let target = match field {
                    0 => &mut stroke.points[0].x,
                    1 => &mut stroke.points[0].y,
                    2 => &mut stroke.points[0].width,
                    3 => &mut stroke.anchors[0].x,
                    4 => &mut stroke.anchors[0].y,
                    5 => &mut stroke.anchors[0].width,
                    6 => &mut stroke.anchors[0].in_x,
                    7 => &mut stroke.anchors[0].in_y,
                    8 => &mut stroke.anchors[0].out_x,
                    9 => &mut stroke.anchors[0].out_y,
                    10 => &mut stroke.hardness,
                    _ => unreachable!(),
                };
                *target = value;
                if save(&path, &invalid).is_ok() {
                    accepted.push((field, value));
                }
                replaced += usize::from(fs::read(&path).unwrap() != original_bytes);
                unreadable += usize::from(load(&path).is_err());
            }
        }
        assert!(
            accepted.is_empty() && replaced == 0 && unreadable == 0,
            "nonfinite vector cases accepted: {accepted:?}; replaced: {replaced}; unreadable: {unreadable}"
        );
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].vector, original.layers[0].vector);
        assert_eq!(loaded.layers[0].pixels.pixel(1, 2), [20, 40, 80, 255]);
    }

    #[test]
    fn efude_save_keeps_existing_file_when_text_exceeds_loading_limits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keep-readable.efude");
        let original = Document::new(8, 8);
        save(&path, &original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let mut accepted = Vec::new();
        let mut replaced = 0;
        let mut unreadable = 0;
        for entry in ["layers.json", "metadata.json"] {
            fs::write(&path, &original_bytes).unwrap();
            let mut oversized = original.clone();
            let text = "x".repeat(16 * 1024 * 1024);
            if entry == "layers.json" {
                oversized.layers[0].name = text;
            } else {
                oversized.metadata.insert("balloons".into(), text);
            }
            if save(&path, &oversized).is_ok() {
                accepted.push(entry);
            }
            replaced += usize::from(fs::read(&path).unwrap() != original_bytes);
            unreadable += usize::from(load(&path).is_err());
        }
        assert!(
            accepted.is_empty() && replaced == 0 && unreadable == 0,
            "oversized entries accepted: {accepted:?}; replaced: {replaced}; unreadable: {unreadable}"
        );
    }

    #[test]
    fn efude_save_checks_vector_byte_limit_before_replacing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keep-vector-budget.efude");
        let original = sample_document();
        save(&path, &original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let with_vectors = small_vector_document();
        let strokes = with_vectors.layers[0].vector.as_ref().unwrap();
        let vector_bytes = serde_json::to_vec(strokes).unwrap().len() as u64;
        let result = save_with_limits(
            &path,
            &with_vectors,
            EfudeLimits {
                vector_bytes: vector_bytes - 1,
                ..EFUDE_LIMITS
            },
        );
        assert!(
            result.is_err() && fs::read(&path).unwrap() == original_bytes,
            "over-budget vector data replaced the existing file"
        );
        assert_eq!(
            load(&path).unwrap().layers[0].pixels.pixel(1, 1),
            [220, 48, 76, 255]
        );
        // The exact boundary must still be writable and readable.
        let exact_limits = EfudeLimits {
            vector_bytes,
            ..EFUDE_LIMITS
        };
        save_with_limits(&path, &with_vectors, exact_limits).unwrap();
        assert_eq!(
            load_with_limits(&path, exact_limits).unwrap().layers[0].vector,
            with_vectors.layers[0].vector
        );
    }

    fn assert_native_budget_boundary(
        document: &Document,
        rejected: EfudeLimits,
        exact: EfudeLimits,
        expected_entries: usize,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keep-native-budget.efude");
        let original = sample_document();
        save(&path, &original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let result = save_with_limits(&path, document, rejected);
        let replaced = fs::read(&path).unwrap() != original_bytes;
        let writer_accepted_but_reader_rejected =
            result.is_ok() && load_with_limits(&path, rejected).is_err();
        assert!(
            result.is_err() && !replaced,
            "writer accepted: {}; replaced: {replaced}; same-cap reader rejected: {writer_accepted_but_reader_rejected}",
            result.is_ok()
        );
        assert_eq!(
            load(&path).unwrap().layers[0].pixels.pixel(1, 1),
            [220, 48, 76, 255]
        );
        save_with_limits(&path, document, exact).unwrap();
        assert_eq!(
            ZipArchive::new(File::open(&path).unwrap()).unwrap().len(),
            expected_entries
        );
        let loaded = load_with_limits(&path, exact).unwrap();
        assert_eq!(
            (loaded.width, loaded.height),
            (document.width, document.height)
        );
        assert_eq!(loaded.metadata, document.metadata);
        assert_eq!(loaded.layers.len(), document.layers.len());
        for (loaded, original) in loaded.layers.iter().zip(&document.layers) {
            assert_eq!(loaded.vector, original.vector);
            assert_eq!(loaded.pixels.to_dense(), original.pixels.to_dense());
            match (&loaded.mask, &original.mask) {
                (Some(loaded), Some(original)) => {
                    assert_eq!(loaded.to_dense(), original.to_dense());
                }
                (None, None) => {}
                _ => panic!("mask lost during budget round trip"),
            }
        }
        match (&loaded.guide, &document.guide) {
            (Some(loaded), Some(original)) => assert_eq!(loaded.rgba, original.rgba),
            (None, None) => {}
            _ => panic!("guide lost during budget round trip"),
        }
    }

    #[test]
    fn efude_save_checks_the_same_tile_slots_as_loading() {
        let mut document = Document::new(TILE_SIZE + 1, 1);
        document
            .layers
            .push(Layer::new(2, "empty", document.width, 1));
        assert_native_budget_boundary(
            &document,
            EfudeLimits {
                tile_slots: 3,
                ..EFUDE_LIMITS
            },
            EfudeLimits {
                tile_slots: 4,
                ..EFUDE_LIMITS
            },
            5,
        );
    }

    #[test]
    fn efude_save_counts_vector_entries_with_all_optional_entries() {
        let mut document = small_vector_document();
        let mut mask = efude_canvas::TilePixels::new(8, 8);
        mask.ensure_tile_filled(0, 0, [255; 4]);
        document.layers[0].mask = Some(mask);
        document
            .metadata
            .insert("fixture".into(), "complete snapshot".into());
        document.guide = GuideImage::fit_to_canvas(8, 8, [11, 22, 33, 255].repeat(64), &document);
        assert_native_budget_boundary(
            &document,
            EfudeLimits {
                archive_entries: 10,
                ..EFUDE_LIMITS
            },
            EfudeLimits {
                archive_entries: 11,
                ..EFUDE_LIMITS
            },
            11,
        );
    }

    #[test]
    fn efude_save_checks_decoded_pixel_tile_memory_before_replacing_file() {
        let mut document = Document::new(TILE_SIZE + 1, 1);
        document.layers[0].pixels.set_pixel(0, 0, [1, 2, 3, 255]);
        document.layers[0]
            .pixels
            .set_pixel(TILE_SIZE, 0, [4, 5, 6, 255]);
        let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
        assert_native_budget_boundary(
            &document,
            EfudeLimits {
                decoded_tile_memory: tile_bytes,
                ..EFUDE_LIMITS
            },
            EfudeLimits {
                decoded_tile_memory: 2 * tile_bytes,
                ..EFUDE_LIMITS
            },
            7,
        );
    }

    #[test]
    fn efude_save_checks_nonuniform_mask_tile_memory_before_replacing_file() {
        let mut document = Document::new(2, 1);
        let mut mask = efude_canvas::TilePixels::new(2, 1);
        mask.set_pixel(0, 0, [0, 0, 0, 255]);
        mask.set_pixel(1, 0, [128, 128, 128, 255]);
        document.layers[0].mask = Some(mask);
        let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
        assert_native_budget_boundary(
            &document,
            EfudeLimits {
                decoded_tile_memory: 0,
                ..EFUDE_LIMITS
            },
            EfudeLimits {
                decoded_tile_memory: tile_bytes,
                ..EFUDE_LIMITS
            },
            6,
        );
    }

    #[test]
    fn efude_save_matches_uniform_mask_allocation_after_partial_tile_cropping() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("uniform-cropped-mask.efude");
        let mut document = Document::new(TILE_SIZE + 1, 1);
        let mut mask = efude_canvas::TilePixels::new(document.width, 1);
        for (tile_x, outside_byte) in [(0, (TILE_SIZE * 4) as usize), (1, 4)] {
            let mut tile = [11, 11, 11, 255].repeat((TILE_SIZE * TILE_SIZE) as usize);
            // Both stored tiles are nonuniform, but the rows/columns written
            // for this canvas crop are uniform and shared by the reader.
            tile[outside_byte] = 99;
            mask.insert_shared_tile(tile_x, 0, std::sync::Arc::new(tile));
        }
        document.layers[0].mask = Some(mask);
        let limits = EfudeLimits {
            decoded_tile_memory: 0,
            ..EFUDE_LIMITS
        };
        save_with_limits(&path, &document, limits).unwrap();
        let loaded = load_with_limits(&path, limits).unwrap();
        let mask = loaded.layers[0].mask.as_ref().unwrap();
        assert_eq!(mask.pixel(0, 0), [11, 11, 11, 255]);
        assert_eq!(mask.pixel(TILE_SIZE, 0), [11, 11, 11, 255]);
        let tiles = mask.tiles().map(|(_, tile)| tile).collect::<Vec<_>>();
        assert_eq!(tiles.len(), 2);
        assert!(std::ptr::eq(tiles[0], tiles[1]));
        assert_eq!(
            ZipArchive::new(File::open(&path).unwrap()).unwrap().len(),
            7
        );
    }

    #[test]
    fn efude_round_trip_preserves_canvas_and_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.efude");
        let original = sample_document();
        save(&path, &original).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!((loaded.width, loaded.height), (4, 3));
        assert_eq!(loaded.layers[0].pixels.pixel(1, 1), [220, 48, 76, 255]);
        assert_eq!(loaded.layers[0].pixels.pixel(2, 1), [18, 90, 240, 127]);
    }

    #[test]
    fn tracing_guide_round_trips_but_never_enters_artwork_exports() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("guided.efude");
        let mut doc = Document::new(4, 3);
        doc.guide = GuideImage::fit_to_canvas(4, 3, [255, 0, 255, 255].repeat(12), &doc);
        doc.layers[0].pixels.set_pixel(1, 1, [0, 0, 0, 255]);
        assert_ne!(efude_canvas::composite_display(&doc, 0), composite(&doc));
        assert_ne!(efude_canvas::composite_with_guide(&doc), composite(&doc));
        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        let guide = loaded.guide.as_ref().unwrap();
        assert_eq!((guide.width, guide.height, guide.opacity), (4, 3, 0.5));
        assert_eq!(
            guide.rgba.as_slice(),
            doc.guide.as_ref().unwrap().rgba.as_slice()
        );
        let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut merged_png = Vec::new();
        archive
            .by_name("merged.png")
            .unwrap()
            .read_to_end(&mut merged_png)
            .unwrap();
        let merged = image::load_from_memory(&merged_png).unwrap().to_rgba8();
        assert_eq!(merged.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(merged.get_pixel(1, 1).0, [0, 0, 0, 255]);
        let png = directory.path().join("art.png");
        export_png(&png, &loaded).unwrap();
        let exported = image::open(&png).unwrap().to_rgba8();
        assert_eq!(exported.get_pixel(0, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn psd_keeps_japanese_layer_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("names.psd");
        let mut document = sample_document();
        document.layers[0].name = "線画 レイヤー1".into();
        export_psd(&path, &document).unwrap();
        let loaded = import_psd(&path).unwrap();
        assert_eq!(loaded.layers[0].name, "線画 レイヤー1");
    }

    fn cmyk_psd_fixture(compression: u16, depth: u16, include_layers: bool) -> Vec<u8> {
        // Photoshop stores CMYK channels inverted; alpha remains uninverted.
        // Four pixels: white, black, red, and red with half opacity.
        let planes = [
            [255u8, 255, 255, 255],
            [255, 255, 0, 0],
            [255, 255, 0, 0],
            [255, 0, 255, 255],
            [255, 255, 255, 128],
        ];
        let rows: Vec<Vec<u8>> = planes
            .iter()
            .map(|plane| {
                let raw = if depth == 8 {
                    plane.to_vec()
                } else {
                    plane
                        .iter()
                        .flat_map(|value| (u16::from(*value) * 257).to_be_bytes())
                        .collect()
                };
                if compression == 1 {
                    let mut encoded = vec![(raw.len() - 1) as u8];
                    encoded.extend_from_slice(&raw);
                    encoded
                } else {
                    raw
                }
            })
            .collect();
        let mut layer_section = Vec::new();
        if include_layers {
            let mut record = Vec::new();
            for bound in [0i32, 0, 1, 4] {
                record.extend_from_slice(&bound.to_be_bytes());
            }
            record.extend_from_slice(&5u16.to_be_bytes());
            let mut data = Vec::new();
            for (channel_id, row) in [0i16, 1, 2, 3, -1].into_iter().zip(&rows) {
                record.extend_from_slice(&channel_id.to_be_bytes());
                record.extend_from_slice(
                    &(2u32 + u32::from(compression == 1) * 2 + row.len() as u32).to_be_bytes(),
                );
                data.extend_from_slice(&compression.to_be_bytes());
                if compression == 1 {
                    data.extend_from_slice(&(row.len() as u16).to_be_bytes());
                }
                data.extend_from_slice(row);
            }
            record.extend_from_slice(b"8BIMnorm");
            record.extend_from_slice(&[255, 0, 0, 0]);
            let mut extra = vec![0; 8];
            extra.extend_from_slice(&[3, b'i', b'n', b'k']);
            record.extend_from_slice(&(extra.len() as u32).to_be_bytes());
            record.extend_from_slice(&extra);
            let mut info = 1u16.to_be_bytes().to_vec();
            info.extend_from_slice(&record);
            info.extend_from_slice(&data);
            if !info.len().is_multiple_of(2) {
                info.push(0);
            }
            layer_section.extend_from_slice(&(info.len() as u32).to_be_bytes());
            layer_section.extend_from_slice(&info);
            layer_section.extend_from_slice(&0u32.to_be_bytes());
        }
        let mut file = b"8BPS".to_vec();
        file.extend_from_slice(&1u16.to_be_bytes());
        file.extend_from_slice(&[0; 6]);
        file.extend_from_slice(&5u16.to_be_bytes());
        file.extend_from_slice(&1u32.to_be_bytes());
        file.extend_from_slice(&4u32.to_be_bytes());
        file.extend_from_slice(&depth.to_be_bytes());
        file.extend_from_slice(&4u16.to_be_bytes());
        file.extend_from_slice(&[0; 8]); // no color-mode data or image resources
        file.extend_from_slice(&(layer_section.len() as u32).to_be_bytes());
        file.extend_from_slice(&layer_section);
        file.extend_from_slice(&compression.to_be_bytes());
        if compression == 1 {
            for row in &rows {
                file.extend_from_slice(&(row.len() as u16).to_be_bytes());
            }
        }
        for row in rows {
            file.extend_from_slice(&row);
        }
        file
    }

    #[test]
    fn psd_cmyk_import_decodes_inverted_color_channels_without_inverting_alpha() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cmyk.psd");
        for compression in [0, 1] {
            for depth in [8, 16] {
                for include_layers in [false, true] {
                    fs::write(&path, cmyk_psd_fixture(compression, depth, include_layers)).unwrap();
                    let report = import_psd_report(&path).unwrap();
                    let pixels = &report.document.layers[0].pixels;
                    for (x, expected) in [
                        [255, 255, 255, 255],
                        [0, 0, 0, 255],
                        [255, 0, 0, 255],
                        [255, 0, 0, 128],
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        assert_eq!(
                            pixels.pixel(x as u32, 0),
                            expected,
                            "compression={compression}, depth={depth}, layers={include_layers}"
                        );
                    }
                    if include_layers {
                        assert_eq!(report.document.layers[0].name, "ink");
                    }
                }
            }
        }
    }

    #[test]
    fn psd_layer_parser_rejects_lengths_outside_their_sections() {
        let file = cmyk_psd_fixture(0, 8, true);
        let section_len = u32::from_be_bytes(file[34..38].try_into().unwrap()) as usize;
        let section = &file[38..38 + section_len];
        // The final raw channel is six bytes. Advertising two more used to
        // seek past the layer-info boundary and still return editable layers.
        let mut bad_channel = section.to_vec();
        bad_channel[50..54].copy_from_slice(&8u32.to_be_bytes());
        assert!(parse_psd_layers(&bad_channel, 4, 1, 4, 8).is_err());

        // A Unicode block has only its four-byte character count, but claims
        // ten bytes. It must be rejected before reserving or reading its units.
        let mut bad_name = section.to_vec();
        let mut block = b"8BIMluni".to_vec();
        block.extend_from_slice(&10u32.to_be_bytes());
        block.extend_from_slice(&0u32.to_be_bytes());
        bad_name.splice(82..82, block);
        let info_len = u32::from_be_bytes(section[..4].try_into().unwrap());
        bad_name[..4].copy_from_slice(&(info_len + 16).to_be_bytes());
        bad_name[66..70].copy_from_slice(&28u32.to_be_bytes());
        assert!(parse_psd_layers(&bad_name, 4, 1, 4, 8).is_err());

        // The block fits, but its declared UTF-16 character is missing.
        bad_name[90..94].copy_from_slice(&4u32.to_be_bytes());
        bad_name[94..98].copy_from_slice(&1u32.to_be_bytes());
        assert!(parse_psd_layers(&bad_name, 4, 1, 4, 8).is_err());
    }

    fn psd_masked_layer_fixture(mask_flags: u8, mask_default: u8, mask_value: u8) -> Vec<u8> {
        let mut record = Vec::new();
        for bound in [1i32, 1, 2, 2] {
            record.extend_from_slice(&bound.to_be_bytes());
        }
        record.extend_from_slice(&5u16.to_be_bytes());
        let mut data = Vec::new();
        for (channel_id, value) in [0i16, 1, 2, -1, -2]
            .into_iter()
            .zip([200u8, 40, 60, 255, mask_value])
        {
            record.extend_from_slice(&channel_id.to_be_bytes());
            record.extend_from_slice(&3u32.to_be_bytes());
            data.extend_from_slice(&0u16.to_be_bytes());
            data.push(value);
        }
        record.extend_from_slice(b"8BIMnorm");
        record.extend_from_slice(&[255, 0, 0, 0]);
        let mut extra = 20u32.to_be_bytes().to_vec();
        for bound in if mask_flags & 1 != 0 {
            [0i32, 0, 1, 1]
        } else {
            [1i32, 1, 2, 2]
        } {
            extra.extend_from_slice(&bound.to_be_bytes());
        }
        extra.extend_from_slice(&[mask_default, mask_flags, 0, 0]);
        extra.extend_from_slice(&0u32.to_be_bytes());
        extra.extend_from_slice(&[3, b'i', b'n', b'k']);
        record.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        record.extend_from_slice(&extra);
        let mut info = 1u16.to_be_bytes().to_vec();
        info.extend_from_slice(&record);
        info.extend_from_slice(&data);
        if !info.len().is_multiple_of(2) {
            info.push(0);
        }
        let mut section = (info.len() as u32).to_be_bytes().to_vec();
        section.extend_from_slice(&info);
        section.extend_from_slice(&0u32.to_be_bytes());
        section
    }

    #[test]
    fn psd_import_respects_disabled_layer_masks() {
        for flags in [2, 3] {
            let (layers, unsupported, _, _) =
                parse_psd_layers(&psd_masked_layer_fixture(flags, 255, 0), 3, 3, 3, 8).unwrap();
            assert!(!unsupported);
            let layers = layers.unwrap();
            assert!(layers[0].mask.is_none(), "disabled mask applied: {flags}");
            assert_eq!(layers[0].pixels.pixel(1, 1), [200, 40, 60, 255]);
        }
    }

    #[test]
    fn psd_import_offsets_layer_relative_masks() {
        for flags in [0, 1] {
            let (layers, unsupported, _, _) =
                parse_psd_layers(&psd_masked_layer_fixture(flags, 255, 0), 3, 3, 3, 8).unwrap();
            assert!(!unsupported);
            let layers = layers.unwrap();
            let mask = layers[0].mask.as_ref().unwrap();
            assert_eq!(mask.pixel(1, 1)[0], 0, "mask coordinate mode: {flags}");
            assert_eq!(mask.pixel(0, 0)[0], 255, "mask coordinate mode: {flags}");
        }
    }

    #[test]
    fn psd_mask_default_tiles_share_storage_until_written() {
        let section = psd_masked_layer_fixture(0, 0, 255);
        let (layers, unsupported, _, _) = parse_psd_layers(&section, 768, 512, 3, 8).unwrap();
        assert!(!unsupported);
        let mut layers = layers.unwrap();
        let mask = layers[0].mask.as_mut().unwrap();
        assert_eq!(mask.pixel(1, 1), [255; 4]);
        assert_eq!(mask.pixel(700, 400), [0; 4]);
        assert_eq!(mask.tile_keys().len(), 6);
        let unique_buffers = mask
            .tiles()
            .map(|(_, data)| data.as_ptr())
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert!(
            unique_buffers <= 2,
            "one default buffer plus one edited tile expected; found {unique_buffers}"
        );
        let original = mask.clone();
        mask.set_pixel(300, 300, [128; 4]);
        assert_eq!(mask.pixel(300, 300), [128; 4]);
        assert_eq!(original.pixel(300, 300), [0; 4]);
        assert_eq!(mask.pixel(700, 400), [0; 4]);
        assert_eq!(mask.pixel(1, 1), [255; 4]);
    }

    fn psd_rgba_layer_fixture(bounds: [i32; 4]) -> Vec<u8> {
        let width = (bounds[3] - bounds[1]) as usize;
        let height = (bounds[2] - bounds[0]) as usize;
        let samples = width * height;
        let mut record = Vec::new();
        for bound in bounds {
            record.extend_from_slice(&bound.to_be_bytes());
        }
        record.extend_from_slice(&4u16.to_be_bytes());
        let mut data = Vec::new();
        for (id, value) in [0i16, 1, 2, -1].into_iter().zip([200u8, 40, 60, 255]) {
            record.extend_from_slice(&id.to_be_bytes());
            record.extend_from_slice(&(2u32 + samples as u32).to_be_bytes());
            data.extend_from_slice(&0u16.to_be_bytes());
            data.extend(std::iter::repeat_n(value, samples));
        }
        record.extend_from_slice(b"8BIMnorm");
        record.extend_from_slice(&[255, 0, 0, 0]);
        record.extend_from_slice(&12u32.to_be_bytes());
        record.extend_from_slice(&[0; 12]);
        let mut info = 1u16.to_be_bytes().to_vec();
        info.extend_from_slice(&record);
        info.extend_from_slice(&data);
        let mut section = (info.len() as u32).to_be_bytes().to_vec();
        section.extend_from_slice(&info);
        section.extend_from_slice(&0u32.to_be_bytes());
        section
    }

    #[test]
    fn psd_tile_budget_rejects_padding_and_clips_to_canvas() {
        let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
        let section = psd_rgba_layer_fixture([0, 0, 1, 768]);
        assert!(parse_psd_layers_with_tile_limit(&section, 768, 1, 3, 8, tile_bytes * 2).is_err());
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(&section, 768, 1, 3, 8, tile_bytes * 3).unwrap();
        let layers = layers.unwrap();
        assert_eq!(layers[0].pixels.tile_keys().len(), 3);
        assert_eq!(layers[0].pixels.pixel(767, 0), [200, 40, 60, 255]);

        let partial = psd_rgba_layer_fixture([0, -256, 1, 512]);
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(&partial, 768, 1, 3, 8, tile_bytes * 2).unwrap();
        let layers = layers.unwrap();
        assert_eq!(layers[0].pixels.tile_keys().len(), 2);
        assert_eq!(layers[0].pixels.pixel(0, 0), [200, 40, 60, 255]);
        assert_eq!(layers[0].pixels.pixel(700, 0), [0; 4]);

        let outside = psd_rgba_layer_fixture([0, 768, 1, 1536]);
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(&outside, 768, 1, 3, 8, 0).unwrap();
        assert!(!layers.unwrap()[0].pixels.has_allocated_tiles());
    }

    #[test]
    fn psd_tile_budget_counts_shared_defaults_and_skips_disabled_masks() {
        let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
        let single = psd_masked_layer_fixture(0, 0, 255);
        let mut info = 2u16.to_be_bytes().to_vec();
        // Two identical records and then both raw channel payloads.
        for _ in 0..2 {
            info.extend_from_slice(&single[6..102]);
        }
        for _ in 0..2 {
            info.extend_from_slice(&single[102..117]);
        }
        let mut section = (info.len() as u32).to_be_bytes().to_vec();
        section.extend_from_slice(&info);
        section.extend_from_slice(&0u32.to_be_bytes());
        assert!(
            parse_psd_layers_with_tile_limit(&section, 768, 512, 3, 8, tile_bytes * 5).is_err()
        );
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(&section, 768, 512, 3, 8, tile_bytes * 6).unwrap();
        for layer in layers.unwrap() {
            let mask = layer.mask.unwrap();
            assert_eq!(mask.pixel(1, 1), [255; 4]);
            assert_eq!(mask.pixel(700, 400), [0; 4]);
        }

        let disabled = psd_masked_layer_fixture(2, 0, 255);
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(&disabled, 768, 512, 3, 8, tile_bytes).unwrap();
        assert!(layers.unwrap()[0].mask.is_none());
        let white = psd_masked_layer_fixture(0, 255, 0);
        assert!(parse_psd_layers_with_tile_limit(&white, 768, 512, 3, 8, tile_bytes).is_err());
        assert!(parse_psd_layers_with_tile_limit(&white, 768, 512, 3, 8, tile_bytes * 2).is_ok());
    }

    #[test]
    fn psd_tile_budget_keeps_cmyk_scratch_separate() {
        let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
        let file = cmyk_psd_fixture(0, 8, true);
        let len = u32::from_be_bytes(file[34..38].try_into().unwrap()) as usize;
        let section = &file[38..38 + len];
        let (layers, _, _, _) =
            parse_psd_layers_with_tile_limit(section, 4, 1, 4, 8, tile_bytes).unwrap();
        assert_eq!(layers.unwrap()[0].pixels.pixel(2, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn psd_import_consumes_folder_end_marker_channels() {
        // Public PSD layer records store all channel payloads after all records,
        // including empty channels on the folder's section-divider record.
        let mut records = Vec::new();
        let mut channel_data = Vec::new();
        for (name, section, empty, rgba) in [
            ("folder", Some(1u32), true, [0; 4]),
            ("child", None, false, [10, 20, 30, 255]),
            ("end", Some(3), true, [0; 4]),
            ("background", None, false, [40, 50, 60, 255]),
        ] {
            for bound in [0i32, 0, i32::from(!empty), i32::from(!empty)] {
                records.extend_from_slice(&bound.to_be_bytes());
            }
            records.extend_from_slice(&4u16.to_be_bytes());
            for (channel, value) in [0i16, 1, 2, -1].into_iter().zip(rgba) {
                records.extend_from_slice(&channel.to_be_bytes());
                records.extend_from_slice(&(if empty { 2u32 } else { 3 }).to_be_bytes());
                channel_data.extend_from_slice(&0u16.to_be_bytes());
                if !empty {
                    channel_data.push(value);
                }
            }
            records.extend_from_slice(b"8BIMnorm");
            records.extend_from_slice(&[255, 0, 0, 0]);
            let mut extra = vec![0; 8]; // no mask or blending ranges
            extra.push(name.len() as u8);
            extra.extend_from_slice(name.as_bytes());
            while !extra.len().is_multiple_of(4) {
                extra.push(0);
            }
            if let Some(section) = section {
                extra.extend_from_slice(b"8BIMlsct");
                extra.extend_from_slice(&4u32.to_be_bytes());
                extra.extend_from_slice(&section.to_be_bytes());
            }
            records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
            records.extend_from_slice(&extra);
        }
        let mut info = 4u16.to_be_bytes().to_vec();
        info.extend_from_slice(&records);
        info.extend_from_slice(&channel_data);
        let mut section = (info.len() as u32).to_be_bytes().to_vec();
        section.extend_from_slice(&info);
        let (layers, unsupported, _, _) = parse_psd_layers(&section, 1, 1, 3, 8).unwrap();
        assert!(!unsupported);
        let layers = layers.unwrap();
        assert_eq!(layers.len(), 3);
        let background = layers.iter().find(|l| l.name == "background").unwrap();
        assert_eq!(background.pixels.pixel(0, 0), [40, 50, 60, 255]);
        let child = layers.iter().find(|l| l.name == "child").unwrap();
        assert_eq!(child.pixels.pixel(0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn psd_round_trip_preserves_nested_folders() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("folders.psd");
        let mut document = sample_document();
        let mut folder = Layer::new(2, "folder", 4, 3);
        folder.kind = LayerKind::Folder;
        folder.expanded = false;
        let mut nested = Layer::new(3, "nested", 4, 3);
        nested.kind = LayerKind::Folder;
        nested.parent_id = Some(2);
        let mut child = Layer::new(4, "child", 4, 3);
        child.parent_id = Some(3);
        child.pixels.set_pixel(0, 0, [1, 2, 3, 255]);
        document.layers.extend([child, nested, folder]);

        export_psd(&path, &document).unwrap();
        let report = import_psd_report(&path).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let loaded = report.document;
        assert_eq!(loaded.layers.len(), 4);
        assert_eq!(composite(&loaded), composite(&document));
        let folder = loaded.layers.iter().find(|l| l.name == "folder").unwrap();
        let nested = loaded.layers.iter().find(|l| l.name == "nested").unwrap();
        let child = loaded.layers.iter().find(|l| l.name == "child").unwrap();
        assert_eq!(folder.kind, LayerKind::Folder);
        assert!(!folder.expanded);
        assert_eq!(nested.parent_id, Some(folder.id));
        assert_eq!(child.parent_id, Some(nested.id));
        assert_eq!(child.pixels.pixel(0, 0), [1, 2, 3, 255]);
    }

    #[test]
    fn psd_round_trip_preserves_masked_layer_names_and_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mask.psd");
        let mut document = sample_document();
        document.layers[0].name = "線画 mask".into();
        let mut mask = efude_canvas::TilePixels::new(4, 3);
        mask.ensure_tile_filled(0, 0, [255; 4]);
        mask.set_pixel(1, 1, [0; 4]);
        mask.set_pixel(2, 1, [128; 4]);
        document.layers[0].mask = Some(mask);

        export_psd(&path, &document).unwrap();
        let report = import_psd_report(&path).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let loaded = report.document;
        assert_eq!(loaded.layers[0].name, document.layers[0].name);
        assert_eq!(composite(&loaded), composite(&document));
        let mask = loaded.layers[0].mask.as_ref().unwrap();
        assert_eq!(mask.pixel(1, 1)[0], 0);
        assert_eq!(mask.pixel(2, 1)[0], 128);
        assert_eq!(mask.pixel(0, 0)[0], 255);
    }

    #[test]
    fn psd_round_trip_preserves_canvas_and_layer_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.psd");
        let original = sample_document();
        export_psd(&path, &original).unwrap();
        let loaded = import_psd(&path).unwrap();
        assert_eq!((loaded.width, loaded.height), (4, 3));
        assert_eq!(loaded.layers[0].pixels.pixel(1, 1), [220, 48, 76, 255]);
        assert_eq!(loaded.layers[0].pixels.pixel(2, 1), [18, 90, 240, 127]);
    }

    #[test]
    fn tone_layers_and_metadata_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.efude");
        let mut doc = Document::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                doc.layers[0].pixels.set_pixel(x, y, [0, 0, 0, 77]);
            }
        }
        doc.layers[0].tone = Some(efude_canvas::ToneSettings {
            lines_per_inch: 85.0,
            shape: efude_canvas::DotShape::Diamond,
            ..Default::default()
        });
        doc.metadata.insert("comic".into(), "{\"page\":1}".into());
        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].tone, doc.layers[0].tone);
        assert_eq!(loaded.metadata, doc.metadata);
        // PSD gets the dots, not the grey density.
        let psd = dir.path().join("tone.psd");
        export_psd(&psd, &doc).unwrap();
        let imported = import_psd(&psd).unwrap();
        let values: std::collections::HashSet<u8> = (0..48)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .map(|(x, y)| imported.layers[0].pixels.pixel(x, y)[3])
            .collect();
        assert!(
            values.contains(&0) && values.contains(&255),
            "tone was not baked: {values:?}"
        );
    }

    #[test]
    fn resized_vector_geometry_stays_aligned_after_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resized-vector.efude");
        let mut doc = Document::new(16, 16);
        doc.layers[0].vector = Some(vec![efude_canvas::VectorStroke::fitted(
            vec![
                efude_canvas::VectorPoint {
                    x: 4.5,
                    y: 6.5,
                    width: 2.0,
                },
                efude_canvas::VectorPoint {
                    x: 10.5,
                    y: 6.5,
                    width: 2.0,
                },
            ],
            [10, 20, 30, 255],
            1.0,
        )]);
        efude_canvas::vector::render_all(&mut doc.layers[0], 16, 16, None);
        efude_canvas::History::default()
            .resize_document(&mut doc, 20, 22, 300.0)
            .unwrap();
        save(&path, &doc).unwrap();
        let mut loaded = load(&path).unwrap();
        assert_eq!((loaded.width, loaded.height), (20, 22));
        assert_eq!(loaded.layers[0].vector, doc.layers[0].vector);
        let point = loaded.layers[0].vector.as_ref().unwrap()[0].points[0];
        assert_eq!((point.x, point.y), (6.5, 9.5));
        let cached = loaded.layers[0].pixels.to_dense();
        assert_eq!(cached, doc.layers[0].pixels.to_dense());
        efude_canvas::vector::render_all(&mut loaded.layers[0], 20, 22, None);
        // Translating f32 coordinates can round antialias coverage by one
        // level; the saved pixels themselves above must round-trip exactly.
        assert!(
            loaded.layers[0]
                .pixels
                .to_dense()
                .iter()
                .zip(&cached)
                .all(|(&rendered, &saved)| rendered.abs_diff(saved) <= 1)
        );
    }

    #[test]
    fn vector_strokes_round_trip_and_folders_load_in_panel_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vector.efude");
        let mut doc = Document::new(64, 48);
        let stroke = efude_canvas::VectorStroke {
            points: vec![
                efude_canvas::VectorPoint {
                    x: 4.0,
                    y: 10.5,
                    width: 3.0,
                },
                efude_canvas::VectorPoint {
                    x: 60.0,
                    y: 10.5,
                    width: 1.5,
                },
            ],
            color: [10, 20, 30, 200],
            hardness: 0.8,
            anchors: Vec::new(),
        };
        // Saved with its curve.
        let stroke = efude_canvas::VectorStroke::fitted(stroke.points, stroke.color, 0.8);
        assert_eq!(stroke.anchors.len(), 2);
        doc.layers[0].vector = Some(vec![stroke.clone()]);
        efude_canvas::vector::render_all(&mut doc.layers[0], 64, 48, None);
        let mut folder = efude_canvas::Layer::new(2, "folder", 64, 48);
        folder.kind = LayerKind::Folder;
        doc.layers.push(folder);
        let mut child = efude_canvas::Layer::new(3, "child", 64, 48);
        child.parent_id = Some(2);
        doc.layers.push(child);
        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].vector, Some(vec![stroke]));
        assert_eq!(
            loaded.layers[0].pixels.to_dense(),
            doc.layers[0].pixels.to_dense()
        );
        let ids = loaded.layers.iter().map(|l| l.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 3, 2]);
    }
}
