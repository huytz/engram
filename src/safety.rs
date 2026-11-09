//! Checks on names that come from the remote: manifest keys, device prefixes, blob shas, plugin names.
use std::path::{Path, PathBuf};

/// Manifest key: relative `/`-separated path, no `.`/`..`/empty segment, no `\`, control chars or NUL
pub fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 1024
        && !k.starts_with('/')
        && !k.contains('\\')
        && !k.chars().any(char::is_control)
        && k.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// Device name: `[A-Za-z0-9._-]{1,64}`, not `.` or `..`
pub fn valid_device(d: &str) -> bool {
    (1..=64).contains(&d.len())
        && d != "."
        && d != ".."
        && d.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Blob sha: 64 lowercase hex chars
pub fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Plugin id as printed in `claude plugin install <id>`
pub fn valid_plugin(p: &str) -> bool {
    (1..=200).contains(&p.len())
        && p.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'-' | b'/' | b':')
        })
}

/// `root/rel` when no existing component below `root` is a symlink; `rel` must pass `valid_key`
pub fn confined(root: &Path, rel: &str) -> Option<PathBuf> {
    if !valid_key(rel) {
        return None;
    }
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return None,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Some(root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        for ok in ["a.md", "sub/x.md", ".hidden", "a..b.md"] {
            assert!(valid_key(ok), "{ok}");
        }
        for bad in [
            "", "/etc/x", "../x", "a/../x", "./x", "a/./x", "a//x", "a/", "a\\x", "a\nb", "..",
        ] {
            assert!(!valid_key(bad), "{bad:?}");
        }
    }

    #[test]
    fn devices() {
        assert!(valid_device("Mac-mini-cua-huytz"));
        for bad in ["", ".", "..", "a b", "a\nINJECT", "a/b", &"x".repeat(65)] {
            assert!(!valid_device(bad), "{bad:?}");
        }
    }

    #[test]
    fn shas() {
        assert!(valid_sha(&"a".repeat(64)));
        assert!(!valid_sha("../../etc/passwd"));
        assert!(!valid_sha(&"A".repeat(64)));
    }

    #[test]
    fn symlink_ancestor_refused() {
        let t = std::env::temp_dir().join(format!("engram-safety-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("root")).unwrap();
        std::os::unix::fs::symlink(&t, t.join("root/link")).unwrap();
        assert!(confined(&t.join("root"), "link/x.md").is_none());
        assert!(confined(&t.join("root"), "ok/x.md").is_some());
        std::fs::remove_dir_all(&t).unwrap();
    }
}
