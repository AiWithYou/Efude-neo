// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
//! Bound the native container's directory before the ZIP reader builds its index.
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zip::ZipArchive;

fn short(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn long(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// Preserve entry metadata verbatim even when it contains EOCD signature bytes.
// A standard ZIP64 footer can encode any validated bounds. Its fixed ZIP32
// end record and maximum-length zero comment leave exactly one EOCD signature
// in zip 9's backward search window, including if the offsets contain one.
struct IndexFooter {
    position: u64,
    bytes: Vec<u8>,
}
impl IndexFooter {
    fn new(start: u64, end: u64, count: u64) -> Self {
        let mut bytes = Vec::with_capacity(98 + usize::from(u16::MAX));
        bytes.extend(b"PK\x06\x06");
        bytes.extend(44u64.to_le_bytes());
        bytes.extend(45u16.to_le_bytes());
        bytes.extend(45u16.to_le_bytes());
        bytes.extend([0; 8]);
        for value in [count, count, end - start, start] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(b"PK\x06\x07");
        bytes.extend([0; 4]);
        bytes.extend(end.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(b"PK\x05\x06");
        bytes.extend([0; 4]);
        bytes.extend([255; 14]); // ZIP64 sentinels and the maximum ZIP32 comment length.
        bytes.resize(bytes.len() + usize::from(u16::MAX), 0);
        Self {
            position: end,
            bytes,
        }
    }
}

// The decoder may retry its footer search while building the index. Only
// the preflighted directory and its footer are visible during that phase.
pub(super) struct NativeArchiveReader {
    file: File,
    position: u64,
    cd_start: u64,
    footer_end: u64,
    index_footer: Option<IndexFooter>,
    indexing_done: Arc<AtomicBool>,
}
impl Read for NativeArchiveReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.indexing_done.load(Ordering::Acquire) {
            let n = self.file.read(buf)?;
            self.position += n as u64;
            return Ok(n);
        }
        let mut buf = buf;
        if let Some(footer) = &self.index_footer {
            if self.position >= footer.position {
                let offset =
                    (self.position - footer.position).min(footer.bytes.len() as u64) as usize;
                let n = (&footer.bytes[offset..]).read(buf)?;
                // Keep the underlying cursor coherent when switching to entry reads.
                self.position = self.file.seek(SeekFrom::Start(self.position + n as u64))?;
                return Ok(n);
            }
            let physical = (footer.position - self.position).min(buf.len() as u64) as usize;
            buf = &mut buf[..physical];
        }
        let n = self.file.read(buf)?;
        let offset = |boundary: u64| boundary.saturating_sub(self.position).min(n as u64) as usize;
        buf[..offset(self.cd_start)].fill(0);
        buf[offset(self.footer_end)..n].fill(0);
        self.position += n as u64;
        Ok(n)
    }
}
impl Seek for NativeArchiveReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let pos = match (&self.index_footer, pos) {
            (Some(footer), SeekFrom::End(delta)) if !self.indexing_done.load(Ordering::Acquire) => {
                let end = footer.position + footer.bytes.len() as u64;
                SeekFrom::Start(
                    end.checked_add_signed(delta)
                        .ok_or(std::io::ErrorKind::InvalidInput)?,
                )
            }
            _ => pos,
        };
        self.position = self.file.seek(pos)?;
        Ok(self.position)
    }
}

pub(super) fn open(
    mut file: File,
    limit: u64,
) -> Result<ZipArchive<NativeArchiveReader>, Box<dyn std::error::Error>> {
    let length = file.seek(SeekFrom::End(0))?;
    // Match the entire EOCD search window used by zip 9, including its extra byte.
    let tail_len = length.min(22 + (1 << 16));
    let tail_start = length - tail_len;
    file.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    let mut error: Box<dyn std::error::Error> = "missing ZIP directory footer".into();
    for (pos, signature) in tail.windows(4).enumerate().rev() {
        if signature != b"PK\x05\x06" {
            continue;
        }
        let e = &tail[pos..];
        if e.len() < 22 || e.len() != 22 + usize::from(short(e, 20)) {
            continue;
        }
        let end = tail_start + pos as u64;
        match validate_directory(&mut file, e, end, limit) {
            Ok((count, start, directory_end)) => {
                // Entry names and extra fields are visible to the indexer too.
                // Pin the footer search if it could otherwise reinterpret their bytes.
                let first = start.saturating_sub(tail_start) as usize;
                let visible = &tail[first..pos + 22];
                let other_signature = visible.windows(4).enumerate().any(|(i, sig)| {
                    let offset = first + i;
                    sig == b"PK\x05\x06" && offset != pos && tail.len() - offset >= 22
                });
                file.rewind()?;
                let indexing_done = Arc::new(AtomicBool::new(false));
                let reader = NativeArchiveReader {
                    file,
                    position: 0,
                    cd_start: start,
                    footer_end: end + 22,
                    index_footer: other_signature
                        .then(|| IndexFooter::new(start, directory_end, count)),
                    indexing_done: Arc::clone(&indexing_done),
                };
                let archive = ZipArchive::new(reader)?;
                if archive.len() as u64 != count || archive.central_directory_start() != start {
                    return Err("ambiguous ZIP directory".into());
                }
                indexing_done.store(true, Ordering::Release);
                return Ok(archive);
            }
            Err(reason) => error = reason,
        }
    }
    Err(error)
}

fn validate_directory(
    file: &mut File,
    e: &[u8],
    mut end: u64,
    limit: u64,
) -> Result<(u64, u64, u64), Box<dyn std::error::Error>> {
    if short(e, 4) != 0 || short(e, 6) != 0 || short(e, 8) != short(e, 10) {
        return Err("invalid single-disk ZIP footer".into());
    }
    let mut count = u64::from(short(e, 10));
    let mut size = u64::from(word(e, 12));
    let mut start = u64::from(word(e, 16));
    if end >= 20 {
        file.seek(SeekFrom::Start(end - 20))?;
        let mut locator = [0; 20];
        file.read_exact(&mut locator)?;
        if &locator[..4] == b"PK\x06\x07" {
            if word(&locator, 4) != 0 || word(&locator, 16) != 1 {
                return Err("unsupported multi-disk ZIP64".into());
            }
            let offset = long(&locator, 8);
            file.seek(SeekFrom::Start(offset))?;
            let mut record = [0; 56];
            file.read_exact(&mut record)?;
            if &record[..4] != b"PK\x06\x06"
                || long(&record, 4) < 44
                || offset
                    .checked_add(12)
                    .and_then(|v| v.checked_add(long(&record, 4)))
                    != Some(end - 20)
                || word(&record, 16) != 0
                || word(&record, 20) != 0
                || long(&record, 24) != long(&record, 32)
            {
                return Err("invalid ZIP64 directory footer".into());
            }
            let fields = [
                (count, long(&record, 32), u64::from(u16::MAX)),
                (size, long(&record, 40), u64::from(u32::MAX)),
                (start, long(&record, 48), u64::from(u32::MAX)),
            ];
            if fields.iter().any(|&(v, wide, max)| v != max && v != wide) {
                return Err("inconsistent ZIP64 directory footer".into());
            }
            (count, size, start, end) = (
                long(&record, 32),
                long(&record, 40),
                long(&record, 48),
                offset,
            );
        }
    }
    if count > limit {
        return Err(".efude archive contains too many entries".into());
    }
    if size > limit.saturating_mul(512).min(64 * 1024 * 1024)
        || start.checked_add(size) != Some(end)
    {
        return Err("invalid or oversized ZIP central directory".into());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut directory = vec![0; size as usize];
    file.read_exact(&mut directory)?;
    let mut remaining = &directory[..];
    let mut names = HashSet::new();
    while !remaining.is_empty() {
        if names.len() as u64 >= limit {
            return Err(".efude archive contains too many entries".into());
        }
        if remaining.len() < 46 || &remaining[..4] != b"PK\x01\x02" {
            return Err("invalid ZIP central directory entry".into());
        }
        let name_end = 46 + usize::from(short(remaining, 28));
        let next = name_end + usize::from(short(remaining, 30)) + usize::from(short(remaining, 32));
        let name = remaining.get(46..name_end).ok_or("truncated ZIP name")?;
        if !names.insert(name) {
            return Err("duplicate ZIP entry name".into());
        }
        remaining = remaining
            .get(next..)
            .ok_or("truncated ZIP directory entry")?;
    }
    if names.len() as u64 != count {
        return Err("ZIP directory count mismatch".into());
    }
    Ok((count, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn directory_index_preserves_incidental_footer_bytes_in_entry_names() {
        let mut writer = zip::ZipWriter::new(tempfile::tempfile().unwrap());
        let name = "note-PK\x05\x06-with-a-long-ordinary-file-name.txt";
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"example").unwrap();
        let mut archive = open(writer.finish().unwrap(), 2).unwrap();
        let mut contents = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "example");
    }

    #[test]
    fn zip_indexer_cannot_open_an_archive_entirely_before_the_allowed_range() {
        let mut writer = zip::ZipWriter::new(tempfile::tempfile().unwrap());
        writer
            .start_file("sample.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"example").unwrap();
        let mut file = writer.finish().unwrap();
        let end = file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(&[0; 22]).unwrap();
        file.rewind().unwrap();
        let reader = NativeArchiveReader {
            file,
            position: 0,
            cd_start: end,
            footer_end: end + 22,
            index_footer: None,
            indexing_done: Arc::new(AtomicBool::new(false)),
        };
        assert!(ZipArchive::new(reader).is_err());
    }

    #[test]
    fn indexing_exposes_only_validated_bytes_across_seeks_and_short_reads() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"bodyINDEXfootercomment").unwrap();
        let indexing_done = Arc::new(AtomicBool::new(false));
        let mut reader = NativeArchiveReader {
            file,
            position: 0,
            cd_start: 4,
            footer_end: 15,
            index_footer: None,
            indexing_done: Arc::clone(&indexing_done),
        };
        for (start, expected) in [
            (0, &b"\0\0\0\0INDEXfooter\0\0\0\0\0\0\0"[..]),
            (2, &b"\0\0IN"[..]),
            (13, &b"er\0\0"[..]),
        ] {
            reader.seek(SeekFrom::Start(start)).unwrap();
            let mut bytes = vec![0; expected.len()];
            reader.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, expected);
        }
        indexing_done.store(true, Ordering::Release);
        reader.rewind().unwrap();
        let mut contents = Vec::new();
        reader.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, b"bodyINDEXfootercomment");
    }
}
