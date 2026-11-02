use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};
fn safe(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|v| v.is_empty() || v == "." || v == "..")
    {
        return Err(denied());
    }
    Ok(())
}
pub(crate) fn normalize(compressed: &[u8]) -> Result<Vec<u8>> {
    if compressed.len() as u64 > protocol::SOURCE_BYTES {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    flate2::read::MultiGzDecoder::new(compressed)
        .take(protocol::SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| denied())?;
    if bytes.len() as u64 > protocol::SOURCE_BYTES {
        return Err(denied());
    }
    // Reject raw extension and trailing records before tar's logical iterator can hide them.
    let mut offset = 0usize;
    let mut records = 0;
    while offset + 512 <= bytes.len() {
        let block = &bytes[offset..offset + 512];
        if block.iter().all(|b| *b == 0) {
            if bytes[offset..].iter().any(|b| *b != 0) {
                return Err(denied());
            }
            break;
        }
        let header = tar::Header::from_byte_slice(block);
        let kind = header.entry_type();
        let size = header.size().map_err(|_| denied())?;
        if !(kind.is_file()
            || kind.is_dir()
            || kind.is_pax_global_extensions()
            || kind.is_pax_local_extensions())
        {
            return Err(denied());
        }
        if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
            if size > 65536 {
                return Err(denied());
            }
            let end = offset
                .checked_add(512)
                .and_then(|n| n.checked_add(size as usize))
                .filter(|n| *n <= bytes.len())
                .ok_or_else(denied)?;
            let metadata = &bytes[offset + 512..end];
            if metadata.windows(10).any(|w| w == b"GNU.sparse")
                || metadata.windows(15).any(|w| w == b"SCHILY.filetype")
            {
                return Err(denied());
            }
        }
        records += 1;
        if records > 50_000 {
            return Err(denied());
        }
        offset = offset
            .checked_add(512)
            .and_then(|n| n.checked_add(size.div_ceil(512).checked_mul(512)? as usize))
            .filter(|n| *n <= bytes.len())
            .ok_or_else(denied)?;
    }
    if offset < bytes.len() && bytes.len() - offset < 512 {
        return Err(denied());
    }
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut root: Option<String> = None;
    let mut files = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for entry in archive.entries().map_err(|_| denied())? {
        let mut entry = entry.map_err(|_| denied())?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|_| denied())?
            .to_str()
            .ok_or_else(denied)?
            .trim_end_matches('/')
            .to_owned();
        safe(&path)?;
        if !names.insert(path.clone()) {
            return Err(denied());
        }
        let (top, rest) = path
            .split_once('/')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((&path, None));
        if root.as_ref().is_some_and(|r| r != top) {
            return Err(denied());
        }
        root = Some(top.into());
        let Some(path) = rest else {
            if !kind.is_dir() {
                return Err(denied());
            }
            continue;
        };
        safe(path)?;
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(denied());
        }
        total = total
            .checked_add(entry.size())
            .filter(|v| *v <= protocol::SOURCE_BYTES)
            .ok_or_else(denied)?;
        let mode = if entry.header().mode().map_err(|_| denied())? & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(|_| denied())?;
        files.insert(path.to_owned(), (mode, content));
    }
    if root.is_none() {
        return Err(denied());
    }
    for path in files.keys() {
        for (index, _) in path.match_indices('/') {
            if files.contains_key(&path[..index]) {
                return Err(denied());
            }
        }
    }
    let mut out = tar::Builder::new(Vec::new());
    for (path, (mode, content)) in files {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        if header.set_path(&path).is_err() {
            out.append_pax_extensions([("path", path.as_bytes())])
                .map_err(|_| denied())?;
            header.set_path("file").map_err(|_| denied())?;
        }
        header.set_cksum();
        out.append(&header, content.as_slice())
            .map_err(|_| denied())?;
    }
    let out = out.into_inner().map_err(|_| denied())?;
    if out.len() as u64 > protocol::SOURCE_BYTES {
        return Err(denied());
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn gzip(raw: &[u8]) -> Vec<u8> {
        let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        out.write_all(raw).unwrap();
        out.finish().unwrap()
    }
    fn tar(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
        let mut out = tar::Builder::new(Vec::new());
        for (name, kind) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_mode(0o6755);
            h.set_size(if kind.is_file() { 1 } else { 0 });
            if kind.is_symlink() {
                h.set_link_name("/outside").unwrap();
            }
            h.set_cksum();
            out.append_data(&mut h, *name, if kind.is_file() { &b"x"[..] } else { &[] })
                .unwrap();
        }
        out.into_inner().unwrap()
    }
    #[test]
    fn normalizes_single_wrapper_and_executable_bits() {
        let result = normalize(&gzip(&tar(&[
            ("root/", tar::EntryType::Directory),
            ("root/run", tar::EntryType::Regular),
        ])))
        .unwrap();
        let mut a = tar::Archive::new(result.as_slice());
        let mut entries = a.entries().unwrap();
        let e = entries.next().unwrap().unwrap();
        assert_eq!(e.path().unwrap().to_str(), Some("run"));
        assert_eq!(e.header().mode().unwrap(), 0o755);
        assert!(entries.next().is_none());
    }
    #[test]
    fn rejects_links_multi_roots_duplicates_collisions_and_trailing_data() {
        for entries in [
            vec![("root/link", tar::EntryType::Symlink)],
            vec![
                ("a/x", tar::EntryType::Regular),
                ("b/y", tar::EntryType::Regular),
            ],
            vec![
                ("a/x", tar::EntryType::Regular),
                ("a/x", tar::EntryType::Regular),
            ],
            vec![
                ("a/x", tar::EntryType::Regular),
                ("a/x/y", tar::EntryType::Regular),
            ],
        ] {
            assert!(normalize(&gzip(&tar(&entries))).is_err());
        }
        let mut bytes = tar(&[("root/x", tar::EntryType::Regular)]);
        bytes.extend_from_slice(b"trailing");
        assert!(normalize(&gzip(&bytes)).is_err());
    }
}
