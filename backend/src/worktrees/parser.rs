use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
    pub is_prunable: bool,
    pub prune_reason: Option<String>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("Invalid worktree snapshot: {0}")]
pub struct ParseError(pub &'static str);

pub fn parse_worktrees(bytes: &[u8]) -> Result<Vec<WorktreeRecord>, ParseError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(b"\0\0") {
        return Err(ParseError("missing NUL record terminator"));
    }
    let mut records = Vec::new();
    let mut fields = Vec::new();
    for field in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if field.is_empty() {
            if fields.is_empty() {
                return Err(ParseError("empty record"));
            }
            records.push(parse_record(&fields)?);
            fields.clear();
        } else {
            fields.push(field);
        }
    }
    if !fields.is_empty() {
        return Err(ParseError("incomplete record"));
    }
    let mut paths = std::collections::HashSet::new();
    if records
        .iter()
        .any(|record| !paths.insert(record.path.clone()))
    {
        return Err(ParseError("duplicate worktree path"));
    }
    Ok(records)
}

fn parse_record(fields: &[&[u8]]) -> Result<WorktreeRecord, ParseError> {
    let first = fields[0]
        .strip_prefix(b"worktree ")
        .ok_or(ParseError("missing worktree path"))?;
    let path = std::str::from_utf8(first).map_err(|_| ParseError("non-UTF8 path"))?;
    if !crate::environment::valid_path(path) {
        return Err(ParseError("path is not absolute"));
    }
    #[cfg(windows)]
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('/', r"\");
    let mut record = WorktreeRecord {
        path: path.into(),
        head: None,
        branch: None,
        is_bare: false,
        is_detached: false,
        is_locked: false,
        lock_reason: None,
        is_prunable: false,
        prune_reason: None,
    };
    let mut seen = std::collections::HashSet::new();
    for field in &fields[1..] {
        let (key, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(index) => (&field[..index], Some(&field[index + 1..])),
            None => (*field, None),
        };
        let known = matches!(
            key,
            b"worktree" | b"HEAD" | b"branch" | b"bare" | b"detached" | b"locked" | b"prunable"
        );
        if known && !seen.insert(key) {
            return Err(ParseError("duplicate field"));
        }
        match key {
            b"worktree" => return Err(ParseError("unexpected worktree field")),
            b"HEAD" => {
                let value = text(value.ok_or(ParseError("missing HEAD"))?)?;
                if !matches!(value.len(), 40 | 64)
                    || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(ParseError("invalid HEAD"));
                }
                record.head = Some(value);
            }
            b"branch" => {
                let value = text(value.ok_or(ParseError("missing branch"))?)?;
                let branch = value
                    .strip_prefix("refs/heads/")
                    .filter(|value| !value.is_empty())
                    .ok_or(ParseError("invalid branch"))?;
                record.branch = Some(branch.into());
            }
            b"bare" if value.is_none() => record.is_bare = true,
            b"detached" if value.is_none() => record.is_detached = true,
            b"bare" | b"detached" => return Err(ParseError("flag has a value")),
            b"locked" => {
                record.is_locked = true;
                record.lock_reason = value.map(text).transpose()?;
            }
            b"prunable" => {
                record.is_prunable = true;
                record.prune_reason = value.map(text).transpose()?;
            }
            _ => {} // Forward-compatible unknown Git fields, including non-UTF8 values.
        }
    }
    if record.is_bare {
        if record.head.is_some() || record.branch.is_some() || record.is_detached {
            return Err(ParseError("inconsistent bare record"));
        }
    } else if record.head.is_none() || record.branch.is_some() == record.is_detached {
        return Err(ParseError("inconsistent HEAD/branch/detached record"));
    }
    Ok(record)
}

fn text(bytes: &[u8]) -> Result<String, ParseError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| ParseError("non-UTF8 field"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    const HEAD: &str = "0123456789012345678901234567890123456789";

    #[test]
    fn main_linked_detached_locked_prunable_and_unknown_fields() {
        let bytes = format!("worktree /tmp/main\0HEAD {HEAD}\0branch refs/heads/main\0future field\0\0worktree /tmp/дерево ' $()\n\0HEAD {HEAD}\0detached\0locked reason with\nnewline\0prunable missing path\0\0");
        let records = parse_worktrees(bytes.as_bytes()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].branch.as_deref(), Some("main"));
        assert_eq!(records[1].path, PathBuf::from("/tmp/дерево ' $()\n"));
        assert!(records[1].is_detached && records[1].is_locked && records[1].is_prunable);
        assert_eq!(
            records[1].lock_reason.as_deref(),
            Some("reason with\nnewline")
        );
        assert_eq!(records[1].prune_reason.as_deref(), Some("missing path"));
    }

    #[test]
    fn bare_unborn_and_reasonless_flags() {
        let bytes = format!("worktree /tmp/bare.git\0bare\0\0worktree /tmp/unborn\0HEAD {}\0branch refs/heads/main\0locked\0prunable\0\0", "0".repeat(40));
        let records = parse_worktrees(bytes.as_bytes()).unwrap();
        assert!(records[0].is_bare);
        assert_eq!(records[0].head, None);
        assert!(records[1].is_locked && records[1].is_prunable);
        assert_eq!(records[1].lock_reason, None);
        assert_eq!(parse_worktrees(b"").unwrap(), Vec::new());
    }

    #[test]
    fn malformed_records_are_rejected() {
        for bytes in [
            b"worktree /tmp/repo\0bare\0".to_vec(),
            b"\0\0".to_vec(),
            b"worktree relative\0bare\0\0".to_vec(),
            b"worktree /tmp/\xff\0bare\0\0".to_vec(),
            b"worktree /tmp/repo\0HEAD invalid\0detached\0\0".to_vec(),
            format!("worktree /tmp/repo\0HEAD {HEAD}\0detached\0branch refs/heads/main\0\0")
                .into_bytes(),
            b"worktree /tmp/repo\0bare\0bare\0\0".to_vec(),
            b"worktree /tmp/repo\0bare\0\0worktree /tmp/repo\0bare\0\0".to_vec(),
        ] {
            assert!(parse_worktrees(&bytes).is_err(), "accepted {bytes:?}");
        }
    }
}
