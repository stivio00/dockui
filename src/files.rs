use std::collections::HashMap;

/// One row of the filesystem explorer.
#[derive(Clone, Debug, PartialEq)]
pub struct FsEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// Parse the tar stream returned by the container archive API into the
/// direct children of the requested directory. Docker prefixes entries
/// with the archived directory's name (`etc/`, `etc/hostname`); when the
/// archive root itself is `/` the entries are top-level names with their
/// contents nested below them. Both shapes are handled by detecting a
/// shared first component that also exists as a standalone entry.
pub fn parse_tar_listing(data: &[u8]) -> Result<Vec<FsEntry>, String> {
    let mut tar = tar::Archive::new(data);
    let mut raw: Vec<(String, bool, u64)> = Vec::new();
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !matches!(entry.header().entry_type(), tar::EntryType::Regular)
            && !matches!(entry.header().entry_type(), tar::EntryType::Directory)
        {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string();
        if path.is_empty() {
            continue;
        }
        let dir = entry.header().entry_type().is_dir();
        let size = if dir { 0 } else { entry.size() };
        raw.push((path, dir, size));
    }
    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let first_comp = |p: &str| p.split('/').next().unwrap_or(p).to_string();
    let head = first_comp(&raw[0].0);
    let shared_prefix =
        raw[0].0 == head && raw.len() > 1 && raw.iter().all(|(p, _, _)| first_comp(p) == head);

    let mut out: HashMap<String, FsEntry> = HashMap::new();
    for (path, dir, size) in raw {
        let rel = if shared_prefix {
            match path.strip_prefix(&format!("{head}/")) {
                Some(rest) => rest,
                None => continue,
            }
        } else {
            &path
        };
        let mut comps = rel.split('/');
        let Some(name) = comps.next() else { continue };
        if comps.next().is_some() {
            out.entry(name.to_string()).or_insert_with(|| FsEntry {
                name: name.to_string(),
                dir: true,
                size: 0,
            });
        } else {
            out.insert(
                name.to_string(),
                FsEntry {
                    name: name.to_string(),
                    dir,
                    size,
                },
            );
        }
    }
    let mut entries: Vec<FsEntry> = out.into_values().collect();
    entries.sort_by(|a, b| {
        a.dir
            .cmp(&b.dir)
            .reverse()
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Deterministic fake filesystem for `--mock` mode.
pub fn mock_listing(path: &str) -> Vec<FsEntry> {
    let e = |name: &str, dir: bool, size: u64| FsEntry {
        name: name.into(),
        dir,
        size,
    };
    match path {
        "/" => vec![
            e("bin", true, 0),
            e("etc", true, 0),
            e("home", true, 0),
            e("usr", true, 0),
            e("var", true, 0),
            e(".dockerenv", false, 0),
        ],
        "/bin" => vec![e("sh", false, 758_144), e("busybox", false, 1_138_112)],
        "/etc" => vec![
            e("hostname", false, 13),
            e("hosts", false, 174),
            e("resolv.conf", false, 92),
            e("ssl", true, 0),
        ],
        "/home" => vec![e("stephen", true, 0)],
        "/usr" => vec![e("bin", true, 0), e("lib", true, 0), e("share", true, 0)],
        "/var" => vec![e("log", true, 0), e("tmp", true, 0)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tar(entries: &[(&str, bool, &str)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, dir, data) in entries {
            let mut h = tar::Header::new_gnu();
            if *dir {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_size(0);
                h.set_mode(0o755);
                h.set_cksum();
                b.append_data(&mut h, path, std::io::empty()).unwrap();
            } else {
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, path, data.as_bytes()).unwrap();
            }
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn parses_docker_subdir_archive() {
        let t = build_tar(&[
            ("etc/", true, ""),
            ("etc/hostname", false, "abc123\n"),
            ("etc/ssl", true, ""),
        ]);
        let out = parse_tar_listing(&t).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].dir, "dirs first: {:?}", out);
        assert_eq!(out[0].name, "ssl");
        assert_eq!(out[1].name, "hostname");
        assert_eq!(out[1].size, 7);
    }

    #[test]
    fn parses_root_archive_without_shared_prefix() {
        let t = build_tar(&[
            ("bin/", true, ""),
            ("bin/dd", false, "x"),
            ("etc/", true, ""),
            ("etc/hostname", false, "abc\n"),
            ("dockerenv", false, ""),
        ]);
        let out = parse_tar_listing(&t).unwrap();
        let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"bin") && names.contains(&"etc") && names.contains(&"dockerenv"));
        assert!(out.iter().find(|e| e.name == "bin").unwrap().dir);
        assert_eq!(out.iter().find(|e| e.name == "dockerenv").unwrap().size, 0);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn empty_tar_is_empty_listing() {
        assert!(parse_tar_listing(&[]).unwrap().is_empty());
    }
}
