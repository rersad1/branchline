use crate::worktrees::SnapshotError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Ordinary,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubmoduleStatus {
    pub commit_changed: bool,
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileStatus {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index_status: String,
    pub worktree_status: String,
    pub kind: FileKind,
    pub similarity: Option<u8>,
    pub submodule: Option<SubmoduleStatus>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GitStatus {
    pub staged: Option<usize>,
    pub unstaged: Option<usize>,
    pub untracked: Option<usize>,
    pub conflicts: Option<usize>,
    pub files: Vec<FileStatus>,
    pub last_successful_refresh_at: Option<DateTime<Utc>>,
    pub error: Option<SnapshotError>,
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid Git status: {0}")]
pub struct StatusParseError(pub &'static str);

pub fn parse_status(bytes: &[u8]) -> Result<GitStatus, StatusParseError> {
    if !bytes.is_empty() && !bytes.ends_with(b"\0") {
        return Err(StatusParseError("missing NUL terminator"));
    }
    let mut fields = bytes
        .strip_suffix(b"\0")
        .unwrap_or(bytes)
        .split(|byte| *byte == 0);
    let mut files = Vec::new();
    if !bytes.is_empty() {
        while let Some(record) = fields.next() {
            if record.is_empty() {
                return Err(StatusParseError("empty record"));
            }
            if record.starts_with(b"# ") {
                continue;
            }
            if let Some(path) = record.strip_prefix(b"? ") {
                files.push(FileStatus {
                    path: file_path(path)?,
                    original_path: None,
                    index_status: "?".into(),
                    worktree_status: "?".into(),
                    kind: FileKind::Untracked,
                    similarity: None,
                    submodule: None,
                });
                continue;
            }
            if let Some(path) = record.strip_prefix(b"! ") {
                file_path(path)?;
                continue;
            }
            let (count, kind, modes, hashes) = match record[0] {
                b'1' => (9, FileKind::Ordinary, 3..6, 6..8),
                b'2' => (10, FileKind::Renamed, 3..6, 6..8),
                b'u' => (11, FileKind::Unmerged, 3..7, 7..10),
                _ => return Err(StatusParseError("unknown record type")),
            };
            let parts: Vec<_> = record.splitn(count, |byte| *byte == b' ').collect();
            if parts.len() != count || parts[0].len() != 1 {
                return Err(StatusParseError("incomplete record"));
            }
            let xy = parts[1];
            if xy.len() != 2 || !xy.iter().all(|byte| b".MADRCUT".contains(byte)) {
                return Err(StatusParseError("invalid XY"));
            }
            if kind == FileKind::Unmerged
                && !matches!(xy, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU")
            {
                return Err(StatusParseError("invalid unmerged XY"));
            }
            for index in modes {
                if parts[index].len() != 6
                    || !parts[index].iter().all(|byte| b"01234567".contains(byte))
                {
                    return Err(StatusParseError("invalid mode"));
                }
            }
            for index in hashes {
                if !matches!(parts[index].len(), 40 | 64)
                    || !parts[index].iter().all(u8::is_ascii_hexdigit)
                {
                    return Err(StatusParseError("invalid object ID"));
                }
            }
            let submodule = submodule(parts[2])?;
            let mut file = FileStatus {
                path: file_path(parts[count - 1])?,
                original_path: None,
                index_status: char::from(xy[0]).to_string(),
                worktree_status: char::from(xy[1]).to_string(),
                kind,
                similarity: None,
                submodule,
            };
            if record[0] == b'2' {
                let score = std::str::from_utf8(parts[8])
                    .map_err(|_| StatusParseError("invalid similarity"))?;
                file.kind = match score.as_bytes().first() {
                    Some(b'R') => FileKind::Renamed,
                    Some(b'C') => FileKind::Copied,
                    _ => return Err(StatusParseError("invalid rename/copy")),
                };
                file.similarity = Some(
                    score[1..]
                        .parse::<u8>()
                        .ok()
                        .filter(|value| *value <= 100)
                        .ok_or(StatusParseError("invalid similarity"))?,
                );
                file.original_path = Some(file_path(
                    fields
                        .next()
                        .ok_or(StatusParseError("missing rename source"))?,
                )?);
            }
            files.push(file);
        }
    }
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    let mut conflicts = 0;
    for file in &files {
        match file.kind {
            FileKind::Unmerged => conflicts += 1,
            FileKind::Untracked => untracked += 1,
            _ => {
                if file.index_status != "." {
                    staged += 1;
                }
                if file.worktree_status != "."
                    || file
                        .submodule
                        .as_ref()
                        .is_some_and(|sub| sub.commit_changed || sub.modified || sub.untracked)
                {
                    unstaged += 1;
                }
            }
        }
    }
    Ok(GitStatus {
        staged: Some(staged),
        unstaged: Some(unstaged),
        untracked: Some(untracked),
        conflicts: Some(conflicts),
        files,
        last_successful_refresh_at: Some(Utc::now()),
        error: None,
    })
}

fn file_path(bytes: &[u8]) -> Result<PathBuf, StatusParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| StatusParseError("non-UTF8 path"))?;
    let path = Path::new(text);
    if text.is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(StatusParseError("invalid relative file path"));
    }
    Ok(path.into())
}

fn submodule(bytes: &[u8]) -> Result<Option<SubmoduleStatus>, StatusParseError> {
    if bytes == b"N..." {
        return Ok(None);
    }
    if bytes.len() != 4
        || bytes[0] != b'S'
        || !b".C".contains(&bytes[1])
        || !b".M".contains(&bytes[2])
        || !b".U".contains(&bytes[3])
    {
        return Err(StatusParseError("invalid submodule field"));
    }
    Ok(Some(SubmoduleStatus {
        commit_changed: bytes[1] == b'C',
        modified: bytes[2] == b'M',
        untracked: bytes[3] == b'U',
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hash() -> String {
        "0".repeat(40)
    }

    #[test]
    fn ordinary_rename_conflict_untracked_and_submodule_counts() {
        let h = hash();
        let bytes = format!(
            "1 MM N... 100644 100644 100644 {h} {h} имя с\nпробелом\0\
            2 R. N... 100644 100644 100644 {h} {h} R100 new name\0old name\0\
            u UU N... 100644 100644 100644 100644 {h} {h} {h} conflict\0\
            ? untracked\0\
            1 .M S.MU 160000 160000 160000 {h} {h} sub\0"
        );
        let status = parse_status(bytes.as_bytes()).unwrap();
        assert_eq!(
            (
                status.staged,
                status.unstaged,
                status.untracked,
                status.conflicts
            ),
            (Some(2), Some(2), Some(1), Some(1))
        );
        assert_eq!(
            status.files[1].original_path.as_deref(),
            Some(Path::new("old name"))
        );
        assert_eq!(status.files[1].similarity, Some(100));
        assert!(status.files[4].submodule.as_ref().unwrap().untracked);
        assert_eq!(parse_status(b"").unwrap().staged, Some(0));
    }

    #[test]
    fn incomplete_unsafe_non_utf8_and_unknown_records_fail() {
        let h = hash();
        for bytes in [
            b"? missing terminator".to_vec(),
            b"? \0".to_vec(),
            b"? ../escape\0".to_vec(),
            b"? /absolute\0".to_vec(),
            b"? \xff\0".to_vec(),
            b"x unknown\0".to_vec(),
            b"1 MM incomplete\0".to_vec(),
            format!("2 R. N... 100644 100644 100644 {h} {h} R100 new\0").into_bytes(),
            format!("1 XX N... 100644 100644 100644 {h} {h} file\0").into_bytes(),
        ] {
            assert!(parse_status(&bytes).is_err(), "accepted {bytes:?}");
        }
    }
}
