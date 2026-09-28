// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Books in the UI: a book file lists page `.efude` files in reading order.
//! Pages open in canvas tabs; the book window shows spreads and exports
//! every page for print (see `docs/spec/comic.md`).

use super::*;
use efude_comic::book::{
    self, Book, BookPage, ExportArea, ExportColor, ExportOptions, Image, NombrePosition,
};
use efude_comic::text::{self, FontInfo, TextStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A running export.
struct ExportJob {
    done: Arc<AtomicUsize>,
    total: usize,
}

#[derive(Clone)]
pub(crate) struct PageSnapshot {
    path: PathBuf,
    document: Document,
    document_id: u64,
    token: u64,
    dirty: bool,
    disk_stamp: Option<(u64, std::time::SystemTime)>,
}

pub(crate) struct SaveReceipt {
    pub path: PathBuf,
    pub document_id: u64,
    pub token: u64,
}

pub(crate) struct ExportRequest {
    book: Book,
    book_path: PathBuf,
    folder: PathBuf,
    options: ExportOptions,
    font: Option<(Arc<Vec<u8>>, u32)>,
    snapshots: std::collections::HashMap<String, PageSnapshot>,
    closed_sources: std::collections::HashMap<String, Option<(u64, std::time::SystemTime)>>,
    pub done: Arc<AtomicUsize>,
    pub save_first: bool,
    overwrite: bool,
    expected: Vec<Option<(u64, std::time::SystemTime)>>,
}

struct ExportPreview {
    request: ExportRequest,
    allow_overwrite: bool,
    collisions: usize,
}

fn path_key(path: &Path) -> String {
    let path = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let key = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

fn output_names(book: &Book, options: &ExportOptions) -> Vec<String> {
    if options.spreads {
        (1..=book.spreads().len())
            .map(|i| format!("spread_{i:03}.png"))
            .collect()
    } else {
        (1..=book.pages.len())
            .map(|i| format!("{i:03}.png"))
            .collect()
    }
}

fn stamp(path: &Path) -> Result<Option<(u64, std::time::SystemTime)>, String> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(Some((
            meta.len(),
            meta.modified().map_err(|e| e.to_string())?,
        ))),
        Ok(_) => Err(format!(
            "出力先が通常のファイルではありません: {}",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// State of the book window.
#[derive(Default)]
pub(crate) struct BookUi {
    pub open: bool,
    /// Book file and its contents.
    book: Option<(PathBuf, Book)>,
    new_title: String,
    new_pages: u32,
    thumbnails: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    export: ExportOptions,
    job: Option<ExportJob>,
    preview: Option<ExportPreview>,
}

impl BookUi {
    pub fn new() -> Self {
        Self {
            new_title: String::new(),
            new_pages: 8,
            ..Self::default()
        }
    }
}

/// A blank page document laid out for page `index` of `book`.
fn blank_page(book: &Book, index: usize) -> Document {
    let spec = book.page_spec(index);
    let g = spec.geometry();
    let mut doc = Document::new(g.canvas_width, g.canvas_height);
    doc.dpi = spec.dpi;
    let comic = comic::ComicDoc {
        page: spec.clone(),
        layout: efude_comic::PanelLayout::for_dpi(spec.dpi),
    };
    doc.metadata.insert(
        "comic".into(),
        serde_json::to_string(&comic).unwrap_or_default(),
    );
    doc
}

fn page_path(book_path: &Path, page: &BookPage) -> PathBuf {
    book_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&page.file)
}

/// Renders page `index` for print: composite with tones, page number,
/// cropped to the export area, colour converted.
fn render_page(
    book: &Book,
    book_path: &Path,
    index: usize,
    options: &ExportOptions,
    font: Option<&(Arc<Vec<u8>>, u32)>,
    snapshots: &std::collections::HashMap<String, PageSnapshot>,
) -> Result<Image, String> {
    let path = page_path(book_path, &book.pages[index]);
    let loaded;
    let doc = if let Some(snapshot) = snapshots.get(&path_key(&path)) {
        &snapshot.document
    } else {
        loaded = efude_io::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        &loaded
    };
    let spec = doc
        .metadata
        .get("comic")
        .and_then(|text| serde_json::from_str::<comic::ComicDoc>(text).ok())
        .map(|comic| comic.page)
        .unwrap_or_else(|| book.page_spec(index));
    let geometry = spec.geometry();
    let mut image = Image {
        width: doc.width,
        height: doc.height,
        rgba: efude_canvas::composite(doc),
    };
    if let (Some(number), Some((data, face))) = (book.nombre_of(index), font) {
        let style = TextStyle {
            size: text::points_to_pixels(book.nombre.size_points, spec.dpi),
            vertical: false,
            line_spacing: 1.0,
            letter_spacing: 0.0,
        };
        if let Some(coverage) = text::render_with(data, *face, &number.to_string(), &style) {
            let origin = book::nombre_origin(
                &geometry,
                book.nombre.position,
                spec.right_page,
                (coverage.width, coverage.height),
            );
            image.draw(&coverage, origin, [0, 0, 0]);
        }
    }
    let mut image = if geometry.canvas_width == doc.width && geometry.canvas_height == doc.height {
        book::page_area(&image, &geometry, options.area, spec.dpi)
    } else {
        image
    };
    book::convert_color(&mut image, options.color, options.threshold);
    Ok(image)
}

fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    image::save_buffer(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

/// Exports every page (or spread) of `book` to `folder`.
fn export_book(request: &ExportRequest) -> Result<PathBuf, String> {
    validate_closed_sources(request)?;
    let ExportRequest {
        book,
        book_path,
        folder,
        options,
        font,
        done,
        snapshots,
        expected,
        ..
    } = request;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let stage = tempfile::Builder::new()
        .prefix(".efude-book-")
        .tempdir_in(folder)
        .map_err(|e| e.to_string())?;
    let names = output_names(book, options);
    if names.len() != expected.len() {
        return Err("出力計画が変わりました".into());
    }
    if options.spreads {
        for (n, (left, right)) in book.spreads().into_iter().enumerate() {
            let left = left
                .map(|i| render_page(book, book_path, i, options, font.as_ref(), snapshots))
                .transpose()?;
            let right = right
                .map(|i| render_page(book, book_path, i, options, font.as_ref(), snapshots))
                .transpose()?;
            if let Some(image) = book::spread(left.as_ref(), right.as_ref()) {
                write_png(
                    &stage.path().join(format!("spread_{:03}.png", n + 1)),
                    &image,
                )?;
            }
            done.fetch_add(
                usize::from(left.is_some()) + usize::from(right.is_some()),
                Ordering::Relaxed,
            );
        }
    } else {
        for index in 0..book.pages.len() {
            let image = render_page(book, book_path, index, options, font.as_ref(), snapshots)?;
            write_png(&stage.path().join(format!("{:03}.png", index + 1)), &image)?;
            done.fetch_add(1, Ordering::Relaxed);
        }
    }
    validate_closed_sources(request)?;
    for (name, expected) in names.iter().zip(expected) {
        if stamp(&folder.join(name))? != *expected {
            return Err(format!(
                "確認後に出力先が変わりました。もう一度確認してください: {name}"
            ));
        }
    }
    publish_pngs(stage, folder, &names, expected)?;
    Ok(folder.clone())
}

fn closed_source_stamps(
    book: &Book,
    path: &Path,
    snapshots: &std::collections::HashMap<String, PageSnapshot>,
) -> Result<std::collections::HashMap<String, Option<(u64, std::time::SystemTime)>>, String> {
    book.pages
        .iter()
        .filter_map(|page| {
            let page = page_path(path, page);
            let key = path_key(&page);
            (!snapshots.contains_key(&key)).then(|| stamp(&page).map(|value| (key, value)))
        })
        .collect()
}

fn validate_closed_sources(request: &ExportRequest) -> Result<(), String> {
    for page in &request.book.pages {
        let path = page_path(&request.book_path, page);
        if let Some(expected) = request.closed_sources.get(&path_key(&path))
            && stamp(&path)? != *expected
        {
            return Err(format!(
                "確認後に閉じたページの原稿が変わりました。内容を更新してください: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Roll back all published files on a publish error, retaining backups if recovery fails.
fn publish_pngs(
    stage: tempfile::TempDir,
    folder: &Path,
    names: &[String],
    expected: &[Option<(u64, std::time::SystemTime)>],
) -> Result<(), String> {
    if expected.len() != names.len() {
        return Err("書き出し先の確認件数が一致しません".into());
    }
    let mut published = Vec::new();
    let mut backups = Vec::new();
    let result: Result<(), String> = (|| {
        let backup_folder = stage.path().join("originals");
        std::fs::create_dir(&backup_folder).map_err(|e| e.to_string())?;
        for (index, name) in names.iter().enumerate() {
            let destination = folder.join(name);
            if stamp(&destination)? != expected[index] {
                return Err(format!(
                    "確認後に出力先が変わりました: {}",
                    destination.display()
                ));
            }
            if expected[index].is_some() {
                let backup = backup_folder.join(name);
                std::fs::rename(&destination, &backup)
                    .map_err(|e| format!("{}: {e}", destination.display()))?;
                backups.push((destination.clone(), backup));
            }
            let source_path = stage.path().join(name);
            let published_stamp = stamp(&source_path)?;
            let source =
                tempfile::TempPath::try_from_path(source_path).map_err(|e| e.to_string())?;
            source
                .persist_noclobber(&destination)
                .map_err(|e| e.to_string())?;
            published.push((destination, published_stamp));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let failures = rollback_pngs(&published, &backups);
        if !failures.is_empty() {
            let recovery = stage.keep();
            return Err(format!(
                "{error}。元のファイルの復元に失敗しました。退避先: {} ({})",
                recovery.display(),
                failures.join("; ")
            ));
        }
        return Err(format!("{error}。出力済みファイルを元に戻しました"));
    }
    Ok(())
}

fn rollback_pngs(
    published: &[(PathBuf, Option<(u64, std::time::SystemTime)>)],
    backups: &[(PathBuf, PathBuf)],
) -> Vec<String> {
    let mut failures = Vec::new();
    for (destination, own_stamp) in published.iter().rev() {
        match stamp(destination) {
            Ok(None) => {}
            Ok(current) if &current == own_stamp => {
                if let Err(e) = std::fs::remove_file(destination) {
                    failures.push(format!("{}: {e}", destination.display()));
                }
            }
            Ok(_) => failures.push(format!(
                "出力後に変更されたファイルを保持しました: {}",
                destination.display()
            )),
            Err(error) => failures.push(error),
        }
    }
    for (destination, backup) in backups.iter().rev() {
        match tempfile::TempPath::try_from_path(backup.clone()) {
            Ok(source) => {
                if let Err(error) = source.persist_noclobber(destination) {
                    let message = format!("{}: {}", destination.display(), error.error);
                    // A failed no-clobber restore must retain the original for recovery.
                    let _ = error.path.keep();
                    failures.push(message);
                }
            }
            Err(error) => failures.push(error.to_string()),
        }
    }
    failures
}

pub(crate) fn execute_export(
    request: ExportRequest,
) -> (Vec<SaveReceipt>, Result<PathBuf, String>) {
    let mut receipts = Vec::new();
    if request.expected.iter().any(Option::is_some) && !request.overwrite {
        return (
            receipts,
            Err("既存PNGの置き換えが確認されていません".into()),
        );
    }
    if request.save_first {
        if let Err(error) = validate_closed_sources(&request) {
            return (receipts, Err(error));
        }
        for snapshot in request.snapshots.values().filter(|s| s.dirty) {
            match stamp(&snapshot.path) {
                Ok(current) if current == snapshot.disk_stamp => {}
                Ok(_) => {
                    return (
                        receipts,
                        Err(format!(
                            "確認後に原稿ファイルが変わりました。保存を止めました: {}",
                            snapshot.path.display()
                        )),
                    );
                }
                Err(error) => return (receipts, Err(error)),
            }
        }
        // Runs on the normal I/O queue; a normal save cannot race this sequence.
        for page in &request.book.pages {
            let path = page_path(&request.book_path, page);
            if let Some(snapshot) = request.snapshots.get(&path_key(&path))
                && snapshot.dirty
            {
                if let Err(error) = efude_io::save(&snapshot.path, &snapshot.document) {
                    return (
                        receipts,
                        Err(format!(
                            "{} の保存に失敗しました。PNGは出力していません: {error}",
                            snapshot.path.display()
                        )),
                    );
                }
                receipts.push(SaveReceipt {
                    path: snapshot.path.clone(),
                    document_id: snapshot.document_id,
                    token: snapshot.token,
                });
            }
        }
    }
    let result = export_book(&request);
    (receipts, result)
}

impl EfudeApp {
    fn save_book(&mut self) {
        if let Some((path, book)) = &self.book_ui.book
            && let Err(error) = std::fs::write(path, book.to_json())
        {
            self.status = format!("{}: {error}", path.display());
        }
    }

    /// Creates a book of `pages` blank pages in `folder`.
    pub(crate) fn create_book(
        &mut self,
        folder: &Path,
        title: &str,
        pages: u32,
        spec: efude_comic::PageSpec,
    ) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let mut book = Book::new(title, spec);
        for n in 1..=pages.max(1) as usize {
            let file = Book::page_file_name(n);
            book.pages.push(BookPage { file: file.clone() });
            let path = folder.join(&file);
            if !path.exists() {
                let doc = blank_page(&book, n - 1);
                efude_io::save(&path, &doc).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        let name = if title.trim().is_empty() {
            "book"
        } else {
            title.trim()
        };
        let path = folder.join(format!("{name}.{}", book::EXTENSION));
        std::fs::write(&path, book.to_json()).map_err(|e| e.to_string())?;
        self.book_ui.book = Some((path, book));
        self.book_ui.thumbnails.clear();
        Ok(())
    }

    pub(crate) fn open_book(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let book = Book::from_json(&text).map_err(|e| e.to_string())?;
        self.book_ui.book = Some((path.to_path_buf(), book));
        self.book_ui.thumbnails.clear();
        Ok(())
    }

    fn add_book_page(&mut self) -> Result<(), String> {
        let Some((path, book)) = &mut self.book_ui.book else {
            return Ok(());
        };
        let folder = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut n = book.pages.len() + 1;
        while folder.join(Book::page_file_name(n)).exists() {
            n += 1;
        }
        let file = Book::page_file_name(n);
        let doc = blank_page(book, book.pages.len());
        efude_io::save(&folder.join(&file), &doc).map_err(|e| e.to_string())?;
        book.pages.push(BookPage { file });
        self.save_book();
        Ok(())
    }

    /// Book pages open in tabs with unsaved changes.
    fn unsaved_book_pages(&self) -> Vec<String> {
        let Some((path, book)) = &self.book_ui.book else {
            return Vec::new();
        };
        let pages: Vec<PathBuf> = book.pages.iter().map(|p| page_path(path, p)).collect();
        self.dirty_tab_paths()
            .into_iter()
            .filter(|p| pages.iter().any(|page| path_key(page) == path_key(p)))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }

    fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        let key = path.to_string_lossy().into_owned();
        if let Some(texture) = self.book_ui.thumbnails.get(&key) {
            return texture.clone();
        }
        let texture = efude_io::load_thumbnail(path).ok().map(|(w, h, rgba)| {
            ctx.load_texture(
                format!("book-thumb-{key}"),
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
                egui::TextureOptions::LINEAR,
            )
        });
        self.book_ui.thumbnails.insert(key, texture.clone());
        texture
    }

    fn prepare_book_export(&mut self, folder: PathBuf) -> Result<(), String> {
        if self.history.is_active()
            || !self.active.is_empty()
            || self.stroke_builder.is_some()
            || self.filter_pending
        {
            return Err("編集中の操作を確定してから書き出してください".into());
        }
        self.commit_pending_guide_edit();
        let Some((path, book)) = self.book_ui.book.clone() else {
            return Err("作品がありません".into());
        };
        if book.pages.is_empty() {
            return Err("作品にページがありません".into());
        }
        let mut page_keys = std::collections::HashSet::new();
        for page in &book.pages {
            if !page_keys.insert(path_key(&page_path(&path, page))) {
                return Err("作品に同じページファイルが複数あります".into());
            }
        }
        let mut snapshots = std::collections::HashMap::new();
        let mut capture =
            |doc: &Document, history: &History, doc_path: &Option<PathBuf>| -> Result<(), String> {
                if let Some(doc_path) = doc_path {
                    let key = path_key(doc_path);
                    if page_keys.contains(&key) {
                        if history.is_active() {
                            return Err("未確定の操作があるページを確定してください".into());
                        }
                        let snapshot = PageSnapshot {
                            path: doc_path.clone(),
                            document: doc.clone(),
                            document_id: history.document_id(),
                            token: history.state_token(),
                            dirty: history.is_dirty(),
                            disk_stamp: stamp(doc_path)?,
                        };
                        if snapshots.insert(key, snapshot).is_some() {
                            return Err(
                                "同じページを複数のタブで開いています。対象を1つにしてください"
                                    .into(),
                            );
                        }
                    }
                }
                Ok(())
            };
        capture(&self.doc, &self.history, &self.doc_path)?;
        for slot in &self.tabs.slots {
            if let Some(tab) = &slot.parked {
                capture(&tab.doc, &tab.history, &tab.doc_path)?;
            }
        }
        let font = self.nombre_font();
        let done = Arc::new(AtomicUsize::new(0));
        let options = self.book_ui.export;
        let expected = output_names(&book, &options)
            .iter()
            .map(|name| stamp(&folder.join(name)))
            .collect::<Result<Vec<_>, _>>()?;
        let collisions = expected.iter().filter(|item| item.is_some()).count();
        let closed_sources = closed_source_stamps(&book, &path, &snapshots)?;
        let request = ExportRequest {
            book,
            book_path: path,
            folder,
            options,
            font,
            snapshots,
            closed_sources,
            done,
            save_first: false,
            overwrite: false,
            expected,
        };
        self.book_ui.preview = Some(ExportPreview {
            request,
            allow_overwrite: false,
            collisions,
        });
        Ok(())
    }

    pub(crate) fn book_export_finished(
        &mut self,
        receipts: Vec<SaveReceipt>,
        result: Result<PathBuf, String>,
    ) {
        for receipt in receipts {
            let mut current_token = None;
            if self.history.document_id() == receipt.document_id
                && self
                    .doc_path
                    .as_ref()
                    .is_some_and(|p| path_key(p) == path_key(&receipt.path))
            {
                current_token = Some(self.history.state_token());
                self.history.mark_saved(receipt.token);
            } else {
                for slot in &mut self.tabs.slots {
                    if let Some(tab) = &mut slot.parked
                        && tab.history.document_id() == receipt.document_id
                        && tab
                            .doc_path
                            .as_ref()
                            .is_some_and(|p| path_key(p) == path_key(&receipt.path))
                    {
                        current_token = Some(tab.history.state_token());
                        tab.history.mark_saved(receipt.token);
                    }
                }
            }
            if let Some(current_token) = current_token
                && let Some(directory) = self.recovery.current_dir().map(Path::to_path_buf)
            {
                let matches_saved = current_token == receipt.token;
                if matches_saved {
                    // Permit a later Undo to create a fresh recovery snapshot of an older state.
                    self.recovery.queued_tokens.remove(&receipt.document_id);
                }
                let _ = self.io_task_sender.send(IoTask::ClearRecovery {
                    directory,
                    document_id: receipt.document_id,
                    saved_token: (!matches_saved).then_some(receipt.token),
                    repaint: egui::Context::default(),
                });
            }
        }
        self.book_ui.job = None;
        self.status = match result {
            Ok(folder) => format!(
                "{}: {}",
                self.text("作品を書き出しました", "Book exported"),
                folder.display()
            ),
            Err(error) => error,
        };
    }

    fn validate_book_save_sources(&self, request: &ExportRequest) -> Result<(), String> {
        for snapshot in request.snapshots.values() {
            let (history, path) = if self.history.document_id() == snapshot.document_id {
                (&self.history, &self.doc_path)
            } else {
                let tab = self
                    .tabs
                    .slots
                    .iter()
                    .filter_map(|slot| slot.parked.as_ref())
                    .find(|tab| tab.history.document_id() == snapshot.document_id)
                    .ok_or("確認したページが閉じられました。内容を更新してください")?;
                (&tab.history, &tab.doc_path)
            };
            if history.is_active()
                || history.state_token() != snapshot.token
                || path
                    .as_ref()
                    .is_none_or(|p| path_key(p) != path_key(&snapshot.path))
            {
                return Err(
                    "確認後にページを変更しました。内容を更新してから保存してください".into(),
                );
            }
        }
        let check_open = |history: &History, path: &Option<PathBuf>| -> Result<(), String> {
            if let Some(path) = path {
                let key = path_key(path);
                if request.closed_sources.contains_key(&key) && history.is_dirty() {
                    return Err(
                        "確認後に開いたページを変更しました。内容を更新してから保存してください"
                            .into(),
                    );
                }
            }
            Ok(())
        };
        check_open(&self.history, &self.doc_path)?;
        for tab in self
            .tabs
            .slots
            .iter()
            .filter_map(|slot| slot.parked.as_ref())
        {
            check_open(&tab.history, &tab.doc_path)?;
        }
        Ok(())
    }

    pub(crate) fn book_export_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut preview) = self.book_ui.preview.take() else {
            return;
        };
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut export = false;
        let mut refresh = false;
        egui::Window::new(t("作品の書き出しを確認", "Review book export"))
            .open(&mut open)
            .default_width(440.0)
            .show(ctx, |ui| {
                let request = &mut preview.request;
                ui.label(format!(
                    "{} · {} {}",
                    request.book.title,
                    request.book.pages.len(),
                    t("ページ", "pages")
                ));
                ui.label(request.folder.display().to_string());
                ui.label(t(
                    "開いているページは、この確認を開いた時点の内容を使います。閉じたページは保存済みの原稿を使います。",
                    "Open pages use contents captured when this review opened. Closed pages use saved documents.",
                ));
                let dirty = request.snapshots.values().filter(|p| p.dirty).count();
                ui.label(format!(
                    "{}: {} / {}",
                    t("未保存の開いたページ", "Open unsaved pages"),
                    dirty,
                    request.snapshots.len()
                ));
                ui.radio_value(
                    &mut request.save_first,
                    false,
                    t(
                        "現在の内容で書き出す（原稿は保存しない）",
                        "Export current contents without saving documents",
                    ),
                );
                ui.radio_value(
                    &mut request.save_first,
                    true,
                    t(
                        "変更したページをすべて保存してから書き出す",
                        "Save all modified pages, then export",
                    ),
                );
                let sources = if request.save_first {
                    self.validate_book_save_sources(request)
                } else {
                    Ok(())
                };
                if let Err(error) = &sources {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                refresh = ui
                    .small_button(t("現在の内容で確認を更新", "Refresh with current contents"))
                    .clicked();
                if preview.collisions > 0 {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!(
                            "{} {}",
                            preview.collisions,
                            t(
                                "個のPNGが同じ名前で存在します",
                                "PNGs already have these names"
                            )
                        ),
                    );
                    ui.checkbox(
                        &mut preview.allow_overwrite,
                        t("確認したPNGを置き換える", "Replace the reviewed PNGs"),
                    );
                }
                egui::CollapsingHeader::new(t("出力ファイル", "Output files")).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for (index, name) in output_names(&request.book, &request.options)
                                .iter()
                                .enumerate()
                            {
                                ui.label(if request.expected[index].is_some() {
                                    format!("{name} · {}", t("置換", "replace"))
                                } else {
                                    name.clone()
                                });
                            }
                        });
                });
                export = ui
                    .add_enabled(
                        sources.is_ok()
                            && (preview.collisions == 0 || preview.allow_overwrite)
                            && self.book_ui.job.is_none(),
                        egui::Button::new(t("この内容で書き出す", "Export these contents")),
                    )
                    .clicked();
            });
        if refresh {
            let save_first = preview.request.save_first;
            let folder = preview.request.folder;
            if let Err(error) = self.prepare_book_export(folder) {
                self.status = error;
            } else if let Some(preview) = &mut self.book_ui.preview {
                preview.request.save_first = save_first;
            }
            return;
        }
        if export {
            if preview.request.save_first
                && let Err(error) = self.validate_book_save_sources(&preview.request)
            {
                self.status = error;
                self.book_ui.preview = Some(preview);
                return;
            }
            preview.request.overwrite = preview.allow_overwrite;
            let total = preview.request.book.pages.len();
            let done = preview.request.done.clone();
            match self.io_task_sender.send(IoTask::BookExport {
                request: preview.request,
                repaint: ctx.clone(),
            }) {
                Ok(()) => {
                    self.book_ui.job = Some(ExportJob { done, total });
                    self.status = t("作品を書き出し中…", "Exporting book…").into();
                }
                Err(error) => self.status = error.to_string(),
            }
            return;
        }
        if open {
            self.book_ui.preview = Some(preview);
        }
    }

    fn nombre_font(&mut self) -> Option<(Arc<Vec<u8>>, u32)> {
        let fonts: Vec<FontInfo> = text::system_fonts();
        let font = [
            "Noto Sans CJK JP",
            "游ゴシック",
            "Yu Gothic",
            "Meiryo",
            "メイリオ",
            "Arial",
            "DejaVu Sans",
        ]
        .iter()
        .find_map(|name| fonts.iter().find(|f| f.name.contains(name)))
        .or(fonts.first())?;
        let data = std::fs::read(&font.path).ok()?;
        Some((Arc::new(data), font.index))
    }

    pub(crate) fn book_window(&mut self, ctx: &egui::Context) {
        if !self.book_ui.open {
            return;
        }
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut action: Option<BookAction> = None;
        if let Some(job) = &self.book_ui.job
            && job.done.load(Ordering::Relaxed) <= job.total
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let book = self.book_ui.book.clone();
        let unsaved = self.unsaved_book_pages();
        egui::Window::new(t("作品（複数ページ）", "Book (Pages)"))
            .id(egui::Id::new("book-window"))
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                let Some((path, book)) = book else {
                    ui.label(t(
                        "作品は、ページごとの .efude ファイルをまとめたものです。フォルダーを選ぶと、原稿の設定（漫画 → 原稿の設定）でページを作ります。",
                        "A book collects one .efude file per page. Choose a folder to create the pages with the current page setup (Manga → Page Setup).",
                    ));
                    egui::Grid::new("book-new").num_columns(2).show(ui, |ui| {
                        ui.label(t("作品名", "Title"));
                        ui.text_edit_singleline(&mut self.book_ui.new_title);
                        ui.end_row();
                        ui.label(t("ページ数", "Pages"));
                        ui.add(egui::DragValue::new(&mut self.book_ui.new_pages).range(1..=400));
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button(t("フォルダーを選んで作成…", "Create in Folder…")).clicked() {
                            action = Some(BookAction::Create);
                        }
                        if ui.button(t("作品を開く…", "Open Book…")).clicked() {
                            action = Some(BookAction::Open);
                        }
                    });
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&book.title).strong().size(16.0));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} · {}",
                            book.pages.len(),
                            t("ページ", "pages"),
                            match book.page.binding {
                                efude_comic::Binding::Right => t("右綴じ", "right-bound"),
                                efude_comic::Binding::Left => t("左綴じ", "left-bound"),
                            }
                        ))
                        .color(layout::MUTED_TEXT),
                    );
                    if ui.small_button(t("閉じる", "Close Book")).clicked() {
                        action = Some(BookAction::CloseBook);
                    }
                });
                ui.separator();
                // Spreads with thumbnails; click a page to open it.
                egui::ScrollArea::vertical()
                    .id_salt("book-spreads")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (left, right) in book.spreads() {
                            ui.horizontal(|ui| {
                                for side in [left, right] {
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(120.0, 170.0),
                                        egui::Sense::click(),
                                    );
                                    let painter = ui.painter();
                                    let Some(index) = side else {
                                        painter.rect_filled(rect.shrink(4.0), 2.0, Color32::from_gray(40));
                                        continue;
                                    };
                                    let page = page_path(&path, &book.pages[index]);
                                    painter.rect_filled(rect.shrink(4.0), 2.0, Color32::WHITE);
                                    if let Some(texture) = self.thumbnail(ctx, &page) {
                                        let size = texture.size_vec2();
                                        let fit = (rect.shrink(6.0).size() / size).min_elem();
                                        let image_rect = Rect::from_center_size(rect.center(), size * fit);
                                        ui.painter().image(
                                            texture.id(),
                                            image_rect,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    }
                                    let label = format!(
                                        "{}{}",
                                        index + 1,
                                        if unsaved.iter().any(|u| u == &book.pages[index].file) {
                                            " ●"
                                        } else {
                                            ""
                                        }
                                    );
                                    ui.painter().text(
                                        rect.center_bottom() + Vec2::new(0.0, -12.0),
                                        egui::Align2::CENTER_CENTER,
                                        label,
                                        egui::FontId::proportional(13.0),
                                        Color32::from_rgb(40, 40, 60),
                                    );
                                    if response
                                        .on_hover_text(t("クリックでタブに開く", "Click to open in a tab"))
                                        .clicked()
                                    {
                                        action = Some(BookAction::OpenPage(index));
                                    }
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button(t("ページを追加", "Add Page")).clicked() {
                        action = Some(BookAction::AddPage);
                    }
                    if ui.button(t("縮小画像を更新", "Refresh Thumbnails")).clicked() {
                        action = Some(BookAction::Refresh);
                    }
                });
                egui::CollapsingHeader::new(t("ページの順番", "Page Order"))
                    .id_salt("book-order")
                    .show(ui, |ui| {
                        for (index, page) in book.pages.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{:>3}  {}  {}",
                                    index + 1,
                                    if book.is_right_page(index) { t("右", "R") } else { t("左", "L") },
                                    page.file
                                ));
                                if ui.add_enabled(index > 0, egui::Button::new("↑").small()).clicked() {
                                    action = Some(BookAction::Move(index, index - 1));
                                }
                                if ui
                                    .add_enabled(index + 1 < book.pages.len(), egui::Button::new("↓").small())
                                    .clicked()
                                {
                                    action = Some(BookAction::Move(index, index + 1));
                                }
                                if ui
                                    .small_button(t("外す", "Remove"))
                                    .on_hover_text(t(
                                        "作品から外します（ファイルは残ります）",
                                        "Removes it from the book (the file stays)",
                                    ))
                                    .clicked()
                                {
                                    action = Some(BookAction::Remove(index));
                                }
                            });
                        }
                    });
                let mut nombre = book.nombre.clone();
                egui::CollapsingHeader::new(t("ノンブル（ページ番号）", "Page Numbers"))
                    .id_salt("book-nombre")
                    .show(ui, |ui| {
                        ui.checkbox(&mut nombre.enabled, t("書き出しに入れる", "Print page numbers"));
                        ui.horizontal(|ui| {
                            ui.label(t("開始番号", "Start at"));
                            ui.add(egui::DragValue::new(&mut nombre.start).range(0..=9999));
                            ui.label(t("大きさ", "Size"));
                            ui.add(egui::DragValue::new(&mut nombre.size_points).range(4.0..=24.0).suffix(" pt"));
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomOuter, t("下・小口側", "Bottom outer"));
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomCenter, t("下・中央", "Bottom centre"));
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomInner, t("下・ノド側", "Bottom inner"));
                        });
                        ui.checkbox(&mut nombre.skip_first, t("最初のページには入れない", "Skip the first page"));
                    });
                if nombre != book.nombre {
                    action = Some(BookAction::Nombre(nombre));
                }
                egui::CollapsingHeader::new(t("書き出し（印刷用）", "Export for Print"))
                    .id_salt("book-export")
                    .default_open(true)
                    .show(ui, |ui| {
                        let export = &mut self.book_ui.export;
                        ui.horizontal(|ui| {
                            ui.label(t("範囲", "Area"));
                            ui.selectable_value(&mut export.area, ExportArea::Trim, t("仕上がり", "Trim"));
                            ui.selectable_value(&mut export.area, ExportArea::Bleed, t("裁ち落としまで", "With bleed"));
                            ui.selectable_value(&mut export.area, ExportArea::WithMarks, t("トンボ付き", "With crop marks"));
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("色", "Colour"));
                            ui.selectable_value(&mut export.color, ExportColor::Color, t("カラー", "Colour"));
                            ui.selectable_value(&mut export.color, ExportColor::Grayscale, t("グレー", "Grey"));
                            ui.selectable_value(&mut export.color, ExportColor::Monochrome, t("モノクロ2階調", "Black and white"));
                        });
                        if export.color == ExportColor::Monochrome {
                            ui.add(egui::Slider::new(&mut export.threshold, 1..=254).text(t("しきい値", "Threshold")));
                        }
                        ui.checkbox(&mut export.spreads, t("見開きで書き出す", "Export spreads"));
                        if !unsaved.is_empty() {
                            ui.colored_label(
                                Color32::from_rgb(255, 205, 110),
                                if english {
                                    format!("Current contents will be exported: {}", unsaved.join(", "))
                                } else {
                                    format!("未保存ページも現在の内容で書き出します: {}", unsaved.join("、"))
                                },
                            );
                        }
                        match &self.book_ui.job {
                            Some(job) => {
                                let done = job.done.load(Ordering::Relaxed);
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                                        .text(format!("{done} / {}", job.total)),
                                );
                            }
                            None => {
                                if ui.button(t("PNGで書き出す…", "Export PNG…")).clicked() {
                                    action = Some(BookAction::Export);
                                }
                            }
                        }
                    });
            });
        match action {
            Some(BookAction::Create) => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let title = if self.book_ui.new_title.trim().is_empty() {
                        folder
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "book".into())
                    } else {
                        self.book_ui.new_title.clone()
                    };
                    let spec = self
                        .comic_doc()
                        .map(|c| c.page)
                        .unwrap_or_else(|| self.comic_ui.setup.clone());
                    if let Err(error) =
                        self.create_book(&folder, &title, self.book_ui.new_pages, spec)
                    {
                        self.status = error;
                    }
                }
            }
            Some(BookAction::Open) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Efude Book", &[book::EXTENSION])
                    .pick_file()
                    && let Err(error) = self.open_book(&path)
                {
                    self.status = error;
                }
            }
            Some(BookAction::CloseBook) => self.book_ui.book = None,
            Some(BookAction::OpenPage(index)) => {
                if let Some((path, book)) = &self.book_ui.book {
                    let page = page_path(path, &book.pages[index]);
                    if let Some(tab) = self.tab_with_path(&page) {
                        self.switch_tab(tab);
                    } else if let Err(error) = self.queue_document_load(page, false, ctx) {
                        self.status = error;
                    }
                }
            }
            Some(BookAction::AddPage) => {
                if let Err(error) = self.add_book_page() {
                    self.status = error;
                }
            }
            Some(BookAction::Refresh) => self.book_ui.thumbnails.clear(),
            Some(BookAction::Move(from, to)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.pages.swap(from, to);
                }
                self.save_book();
            }
            Some(BookAction::Remove(index)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.pages.remove(index);
                }
                self.save_book();
            }
            Some(BookAction::Nombre(nombre)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.nombre = nombre;
                }
                self.save_book();
            }
            Some(BookAction::Export) => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder()
                    && let Err(error) = self.prepare_book_export(folder)
                {
                    self.status = error;
                }
            }
            None => {}
        }
        self.book_ui.open = open;
    }
}

enum BookAction {
    Create,
    Open,
    CloseBook,
    OpenPage(usize),
    AddPage,
    Refresh,
    Move(usize, usize),
    Remove(usize),
    Nombre(efude_comic::book::Nombre),
    Export,
}

#[cfg(test)]
pub(crate) fn export_for_test(
    book_path: &Path,
    folder: &Path,
    options: ExportOptions,
) -> Result<PathBuf, String> {
    let book = Book::from_json(&std::fs::read_to_string(book_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let fonts = text::system_fonts();
    let font = fonts
        .first()
        .and_then(|f| std::fs::read(&f.path).ok().map(|d| (Arc::new(d), f.index)));
    let expected = output_names(&book, &options)
        .iter()
        .map(|name| stamp(&folder.join(name)))
        .collect::<Result<Vec<_>, _>>()?;
    let closed_sources = closed_source_stamps(&book, book_path, &Default::default())?;
    export_book(&ExportRequest {
        book,
        book_path: book_path.to_path_buf(),
        folder: folder.to_path_buf(),
        options,
        font,
        done: Arc::new(AtomicUsize::new(0)),
        snapshots: Default::default(),
        closed_sources,
        save_first: false,
        overwrite: true,
        expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn small_book(root: &Path) -> (PathBuf, Book) {
        let path = root.join("test.efudebook");
        let mut book = Book::new("Test", efude_comic::PageSpec::presets().remove(0).2);
        book.nombre.enabled = false;
        book.pages = vec![
            BookPage {
                file: "001.efude".into(),
            },
            BookPage {
                file: "002.efude".into(),
            },
        ];
        for page in &book.pages {
            efude_io::save(&page_path(&path, page), &Document::new(8, 8)).unwrap();
        }
        (path, book)
    }

    #[test]
    fn active_and_parked_unsaved_snapshots_export_without_saving_source_files() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), page_path(&path, &book.pages[0]));
        app.doc.layers[0].pixels.set_pixel(1, 1, [255, 0, 0, 255]);
        app.add_raster_layer(); // makes this tab dirty
        let first_id = app.history.document_id();
        app.install_document(Document::new(8, 8), page_path(&path, &book.pages[1]));
        app.doc.layers[0].pixels.set_pixel(2, 2, [0, 0, 255, 255]);
        app.add_raster_layer();
        app.book_ui.book = Some((path.clone(), book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        app.prepare_book_export(folder.clone()).unwrap();
        let request = app.book_ui.preview.take().unwrap().request;
        assert_eq!(request.snapshots.len(), 2);
        assert!(
            request
                .snapshots
                .values()
                .any(|s| s.document_id == first_id)
        );
        // A later edit must neither enter the frozen output nor be marked saved.
        app.doc.layers[0].pixels.set_pixel(2, 2, [0, 255, 0, 255]);
        let (receipts, result) = execute_export(request);
        assert!(receipts.is_empty());
        result.unwrap();
        assert_eq!(
            image::open(folder.join("001.png"))
                .unwrap()
                .to_rgba8()
                .get_pixel(1, 1)
                .0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            image::open(folder.join("002.png"))
                .unwrap()
                .to_rgba8()
                .get_pixel(2, 2)
                .0,
            [0, 0, 255, 255]
        );
        assert!(
            efude_io::load(&directory.path().join("002.efude"))
                .unwrap()
                .layers[0]
                .pixels
                .tiles()
                .next()
                .is_none()
        );
        assert!(app.history.is_dirty());
    }

    #[test]
    fn render_failure_leaves_existing_pngs_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        std::fs::remove_file(page_path(&path, &book.pages[1])).unwrap();
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("001.png"), b"original").unwrap();
        let options = ExportOptions::default();
        let expected = output_names(&book, &options)
            .iter()
            .map(|name| stamp(&folder.join(name)).unwrap())
            .collect::<Vec<_>>();
        assert!(
            export_book(&ExportRequest {
                book,
                book_path: path,
                folder: folder.clone(),
                options,
                font: None,
                done: Arc::new(AtomicUsize::new(0)),
                snapshots: Default::default(),
                closed_sources: Default::default(),
                save_first: false,
                overwrite: true,
                expected
            })
            .is_err()
        );
        assert_eq!(std::fs::read(folder.join("001.png")).unwrap(), b"original");
        assert!(!folder.join("002.png").exists());
    }

    #[test]
    fn publish_failure_restores_previous_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path();
        std::fs::write(folder.join("001.png"), b"original").unwrap();
        let stage = tempfile::tempdir_in(folder).unwrap();
        std::fs::write(stage.path().join("001.png"), b"new").unwrap();
        // Missing second staged output forces a failure after the first publication.
        let names = vec!["001.png".into(), "002.png".into()];
        let expected = vec![stamp(&folder.join("001.png")).unwrap(), None];
        assert!(publish_pngs(stage, folder, &names, &expected).is_err());
        assert_eq!(std::fs::read(folder.join("001.png")).unwrap(), b"original");
        assert!(!folder.join("002.png").exists());
    }

    #[test]
    fn saving_after_review_cannot_be_overwritten_by_the_older_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let mut app = EfudeApp::default();
        let page = page_path(&path, &book.pages[0]);
        app.install_document(Document::new(8, 8), page.clone());
        app.add_raster_layer();
        app.book_ui.book = Some((path, book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        app.prepare_book_export(folder.clone()).unwrap();
        let mut request = app.book_ui.preview.take().unwrap().request;
        request.save_first = true;
        app.add_raster_layer();
        assert!(app.validate_book_save_sources(&request).is_err());
        efude_io::save(&page, &app.doc).unwrap();
        let saved = std::fs::read(&page).unwrap();
        let (receipts, result) = execute_export(request);
        assert!(result.is_err());
        assert!(receipts.is_empty());
        assert_eq!(std::fs::read(page).unwrap(), saved);
        assert!(!folder.join("001.png").exists());
    }

    #[test]
    fn a_later_save_failure_keeps_receipts_and_exports_no_pngs() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let first_path = page_path(&path, &book.pages[0]);
        let second_path = page_path(&path, &book.pages[1]);
        let second_before = std::fs::read(&second_path).unwrap();
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), first_path.clone());
        app.add_raster_layer();
        app.install_document(Document::new(8, 8), second_path.clone());
        app.add_raster_layer();
        app.book_ui.book = Some((path, book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        app.prepare_book_export(folder.clone()).unwrap();
        let mut request = app.book_ui.preview.take().unwrap().request;
        request.save_first = true;
        // Forces a real document-save error after the preceding page has saved.
        request
            .snapshots
            .get_mut(&path_key(&second_path))
            .unwrap()
            .document
            .dpi = f32::NAN;
        let (receipts, result) = execute_export(request);
        assert!(result.is_err());
        assert_eq!(receipts.len(), 1);
        assert_eq!(path_key(&receipts[0].path), path_key(&first_path));
        assert_eq!(efude_io::load(&first_path).unwrap().layers.len(), 2);
        assert_eq!(std::fs::read(&second_path).unwrap(), second_before);
        assert!(!folder.join("001.png").exists());
        assert!(!folder.join("002.png").exists());
        app.book_export_finished(receipts, result);
        assert!(app.history.is_dirty());
        let parked = app
            .tabs
            .slots
            .iter()
            .filter_map(|s| s.parked.as_ref())
            .find(|t| {
                t.doc_path
                    .as_ref()
                    .is_some_and(|p| path_key(p) == path_key(&first_path))
            })
            .unwrap();
        assert!(!parked.history.is_dirty());
    }

    #[test]
    fn rollback_preserves_outputs_changed_by_another_writer_and_original_backups() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("001.png");
        let backup = directory.path().join("original.png");
        std::fs::write(&backup, b"original").unwrap();
        std::fs::write(&destination, b"our output").unwrap();
        let published = vec![(destination.clone(), stamp(&destination).unwrap())];
        std::fs::write(&destination, b"edited by another writer").unwrap();
        let errors = rollback_pngs(&published, &[(destination.clone(), backup.clone())]);
        assert!(!errors.is_empty());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"edited by another writer"
        );
        assert_eq!(std::fs::read(&backup).unwrap(), b"original");
    }

    #[test]
    fn save_receipts_mark_only_the_saved_token_and_the_same_path() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let mut app = EfudeApp::default();
        let page = page_path(&path, &book.pages[0]);
        app.install_document(Document::new(8, 8), page);
        app.add_raster_layer();
        app.book_ui.book = Some((path, book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        app.prepare_book_export(folder.clone()).unwrap();
        let mut request = app.book_ui.preview.take().unwrap().request;
        request.save_first = true;
        app.validate_book_save_sources(&request).unwrap();
        let (receipts, result) = execute_export(request);
        assert_eq!(receipts.len(), 1);
        result.as_ref().unwrap();
        app.add_raster_layer(); // a later edit must remain dirty
        app.book_export_finished(receipts, result);
        assert!(app.history.is_dirty());
        app.history.undo_document(&mut app.doc);
        assert!(!app.history.is_dirty());
    }

    #[test]
    fn unreviewed_overwrite_is_rejected_before_any_saves() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), page_path(&path, &book.pages[0]));
        app.add_raster_layer();
        app.book_ui.book = Some((path, book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("001.png"), b"original").unwrap();
        app.prepare_book_export(folder.clone()).unwrap();
        let mut request = app.book_ui.preview.take().unwrap().request;
        request.save_first = true;
        let (receipts, result) = execute_export(request);
        assert!(result.is_err());
        assert!(receipts.is_empty());
        assert_eq!(std::fs::read(folder.join("001.png")).unwrap(), b"original");
    }

    #[test]
    fn save_review_detects_new_edits_in_previously_clean_or_closed_pages() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let first = page_path(&path, &book.pages[0]);
        let second = page_path(&path, &book.pages[1]);
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), first.clone());
        app.book_ui.book = Some((path, book));
        let output = directory.path().join("output");
        app.prepare_book_export(output.clone()).unwrap();
        let request = app.book_ui.preview.take().unwrap().request;
        assert!(!request.snapshots.values().next().unwrap().dirty);
        app.add_raster_layer();
        assert!(app.validate_book_save_sources(&request).is_err());
        app.history.undo_document(&mut app.doc);
        app.validate_book_save_sources(&request).unwrap();
        app.install_document(Document::new(8, 8), second);
        app.add_raster_layer();
        assert!(app.validate_book_save_sources(&request).is_err());
    }

    #[test]
    fn changed_closed_source_is_rejected_before_saving_any_open_page() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let first = page_path(&path, &book.pages[0]);
        let second = page_path(&path, &book.pages[1]);
        let original = std::fs::read(&first).unwrap();
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), first.clone());
        app.add_raster_layer();
        app.book_ui.book = Some((path, book));
        let output = directory.path().join("output");
        app.prepare_book_export(output.clone()).unwrap();
        let mut request = app.book_ui.preview.take().unwrap().request;
        request.save_first = true;
        std::fs::write(second, b"changed outside the app").unwrap();
        let (receipts, result) = execute_export(request);
        assert!(result.is_err());
        assert!(receipts.is_empty());
        assert_eq!(std::fs::read(first).unwrap(), original);
        assert!(!output.join("001.png").exists());
    }

    #[test]
    fn batch_save_clears_saved_recovery_and_keeps_a_later_edit_recoverable() {
        fn drain(app: &mut EfudeApp, ctx: &egui::Context) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while app.io_task_sender.busy.get() > 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1280.0, 820.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.update_ui(ctx),
                );
            }
            assert_eq!(app.io_task_sender.busy.get(), 0);
        }
        for edit_after_save in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (path, book) = small_book(directory.path());
            let mut app = EfudeApp::default();
            app.install_document(Document::new(8, 8), page_path(&path, &book.pages[0]));
            app.add_raster_layer();
            let ctx = egui::Context::default();
            let id = app.history.document_id();
            let saved_token = app.history.state_token();
            app.queue_recovery_snapshots(&ctx).unwrap();
            drain(&mut app, &ctx);
            let old = app.recovery.snapshot_paths(id, saved_token).unwrap().0;
            assert!(old.exists());
            app.book_ui.book = Some((path, book));
            app.prepare_book_export(directory.path().join("output"))
                .unwrap();
            let mut request = app.book_ui.preview.take().unwrap().request;
            request.save_first = true;
            let (receipts, result) = execute_export(request);
            result.as_ref().unwrap();
            if edit_after_save {
                app.add_raster_layer();
                app.queue_recovery_snapshots(&ctx).unwrap();
                drain(&mut app, &ctx);
            }
            let current = app
                .recovery
                .snapshot_paths(id, app.history.state_token())
                .unwrap()
                .0;
            app.book_export_finished(receipts, result);
            drain(&mut app, &ctx);
            assert!(!old.exists());
            assert_eq!(current.exists(), edit_after_save);
            assert_eq!(app.history.is_dirty(), edit_after_save);
            assert_eq!(
                app.recovery.queued_tokens.contains_key(&id),
                edit_after_save
            );
        }
    }

    #[test]
    fn review_ui_requires_overwrite_choice_and_refresh_before_saving_changed_pages() {
        let directory = tempfile::tempdir().unwrap();
        let (path, book) = small_book(directory.path());
        let page = page_path(&path, &book.pages[0]);
        let mut h = crate::tool_tests::Harness::new(8, 8);
        h.app.install_document(Document::new(8, 8), page.clone());
        h.app.add_raster_layer();
        h.app.book_ui.book = Some((path, book));
        let folder = directory.path().join("output");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("001.png"), b"original").unwrap();
        h.app.prepare_book_export(folder.clone()).unwrap();
        h.frames(3);
        h.click_label("この内容で書き出す");
        assert!(h.app.book_ui.preview.is_some());
        assert!(h.app.book_ui.job.is_none());
        h.click_label("確認したPNGを置き換える");
        h.click_label("変更したページをすべて保存してから書き出す");
        h.app.add_raster_layer();
        h.frames(2);
        h.click_label("この内容で書き出す");
        assert!(h.app.book_ui.preview.is_some());
        assert!(h.app.book_ui.job.is_none());
        assert_eq!(std::fs::read(folder.join("001.png")).unwrap(), b"original");
        h.click_label("現在の内容で確認を更新");
        assert!(h.app.book_ui.preview.as_ref().unwrap().request.save_first);
        assert!(!h.app.book_ui.preview.as_ref().unwrap().allow_overwrite);
        h.click_label("確認したPNGを置き換える");
        h.click_label("この内容で書き出す");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while h.app.book_ui.job.is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            h.frames(1);
        }
        assert!(h.app.book_ui.preview.is_none());
        assert!(h.app.book_ui.job.is_none(), "{}", h.app.status);
        assert!(!h.app.history.is_dirty(), "{}", h.app.status);
        assert_eq!(efude_io::load(&page).unwrap().layers.len(), 3);
        assert_ne!(std::fs::read(folder.join("001.png")).unwrap(), b"original");
    }
}
