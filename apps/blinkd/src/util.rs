//! Small shared helpers: identifier sanitisation, filesystem-safe path joins,
//! and unit variable templating.

use std::path::{Component, Path, PathBuf};

/// Replace anything outside `[a-zA-Z0-9_.-]` with `_` (volume/container names).
/// Guards against the result being `.` or `..`, which would escape the
/// volumes directory.
pub fn sanitize_volume_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
        .collect();
    match sanitized.as_str() {
        "" | "." | ".." => "_".to_string(),
        _ => sanitized,
    }
}

/// Console session IDs allow only `[a-zA-Z0-9-_]`.
pub fn sanitize_server_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect()
}

/// Join `requested` onto `base`, refusing anything that escapes `base`.
///
/// Unlike a naive `starts_with` check on the joined string, this uses
/// component-wise comparison after lexical normalisation, so a sibling
/// directory named `server2` can never be reached from a server rooted at
/// `server`. Symlinks that resolve outside the base are rejected by callers
/// via `canonicalize_within`.
pub fn safe_join(base: &Path, requested: &str) -> Option<PathBuf> {
    let requested = requested.trim();
    let requested = requested.strip_prefix('/').unwrap_or(requested);

    let rel = Path::new(requested);
    let mut cleaned = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            // Any `..` or absolute/root component escapes the base: reject.
            _ => return None,
        }
    }

    Some(base.join(cleaned))
}

/// Canonicalise `path` and assert it still lives inside `base`.
#[allow(dead_code)]
///
/// `base` must already be canonical. This is the second half of the traversal
/// defence and also catches symlinks pointing outside the volume.
pub fn canonicalize_within(base: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    if canonical.starts_with(base) {
        Some(canonical)
    } else {
        None
    }
}

/// Combined guard used by filesystem handlers: lexical join + symlink check.
pub fn resolve_within(base: &Path, requested: &str) -> Option<PathBuf> {
    let joined = safe_join(base, requested)?;
    // Canonicalise the deepest existing ancestor, then re-attach the tail, so
    // writes to not-yet-existing files still resolve through real symlinks.
    match std::fs::canonicalize(&joined) {
        Ok(canon) => {
            if canon.starts_with(base) {
                Some(canon)
            } else {
                None
            }
        }
        Err(_) => {
            // Path (partially) missing: canonicalise the existing parent chain.
            let mut existing = joined.clone();
            let mut tail = Vec::new();
            loop {
                if std::fs::symlink_metadata(&existing).is_ok() {
                    break;
                }
                {
                    let p = existing.parent()?;
                    tail.push(existing.file_name()?.to_os_string());
                    existing = p.to_path_buf();
                }
            }
            let canon_parent = std::fs::canonicalize(&existing).ok()?;
            if !canon_parent.starts_with(base) {
                return None;
            }
            let mut resolved = canon_parent;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            if resolved.starts_with(base) {
                Some(resolved)
            } else {
                None
            }
        }
    }
}

/// Convert a variable name to its placeholder form: `Server Jar` -> `%server_jar%`.
pub fn variable_placeholder(name: &str) -> String {
    format!("%{}%", name.to_lowercase().replace(' ', "_"))
}

/// Validate a variable value against Pterodactyl-style pipe rules.
/// Supported: `nullable`, `string`, `max:N`.
pub fn validate_variable_value(value: &str, rules: &str) -> bool {
    for rule in rules.split('|') {
        match rule.trim() {
            "nullable" => {
                if value.is_empty() {
                    return true;
                }
            }
            "string" => {}
            r if r.starts_with("max:") => {
                if let Ok(max) = r[4..].parse::<usize>() {
                    if value.chars().count() > max {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// Apply `%variable%` and `%cargo:['path']%` substitution to a startup command.
pub fn process_variables(
    input: &str,
    variables: &[crate::models::ServerVariable],
    cargo: Option<&[crate::models::CargoFile]>,
) -> Result<String, String> {
    let mut result = input.to_string();

    for variable in variables {
        let value = variable.current_value.clone().unwrap_or_else(|| variable.default_value.clone());
        if !validate_variable_value(&value, &variable.rules) {
            return Err(format!(
                "Variable {} value doesn't match rules: {}",
                variable.name, variable.rules
            ));
        }
        result = result.replace(&variable_placeholder(&variable.name), &value);
    }

    if let Some(cargo) = cargo {
        // Replace %cargo:['path']% placeholders with the target path itself.
        let mut out = String::with_capacity(result.len());
        let mut rest = result.as_str();
        while let Some(idx) = rest.find("%cargo:['") {
            out.push_str(&rest[..idx]);
            let after = &rest[idx + 9..];
            match after.find("']%") {
                Some(end) => {
                    let path = &after[..end];
                    if !cargo.iter().any(|c| c.target_path == path) {
                        return Err(format!("Referenced cargo file not found: {path}"));
                    }
                    out.push_str(path);
                    rest = &after[end + 3..];
                }
                None => {
                    out.push_str(&rest[..idx + 9]);
                    rest = &after[..0];
                    break;
                }
            }
        }
        out.push_str(rest);
        result = out;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_volume_name("abc-123_DEF.x"), "abc-123_DEF.x");
        assert_eq!(sanitize_volume_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_volume_name(".."), "_");
        assert_eq!(sanitize_volume_name("."), "_");
        assert_eq!(sanitize_volume_name(""), "_");
        assert_eq!(sanitize_server_id("abc<script>"), "abcscript");
    }

    #[test]
    fn safe_join_blocks_traversal() {
        let base = PathBuf::from("/srv/volumes/server1");
        assert_eq!(safe_join(&base, "plugins/a.jar"), Some(base.join("plugins/a.jar")));
        assert_eq!(safe_join(&base, "."), Some(base.clone()));
        assert_eq!(safe_join(&base, "../server2/world"), None);
        assert_eq!(safe_join(&base, ".."), None);
        // Leading slashes are treated as relative to the server root
        // (same as node's path.join semantics used by the original daemon).
        assert_eq!(safe_join(&base, "/etc/passwd"), Some(base.join("etc/passwd")));
        assert_eq!(safe_join(&base, "a/../../b"), None);
    }

    #[test]
    fn safe_join_not_vulnerable_to_prefix_siblings() {
        // The classic `startsWith` bug: /base/server vs /base/server2.
        let base = PathBuf::from("/srv/volumes/server");
        let joined = safe_join(&base, "../server2/file").map(|p| p.to_string_lossy().to_string());
        assert_eq!(joined, None);
    }

    #[test]
    fn process_variables_standard() {
        let vars = vec![crate::models::ServerVariable {
            name: "Server Jar".into(),
            description: String::new(),
            current_value: Some("paper.jar".into()),
            default_value: "server.jar".into(),
            rules: "string".into(),
        }];
        let out = process_variables("java -jar %server_jar%", &vars, None).unwrap();
        assert_eq!(out, "java -jar paper.jar");
    }

    #[test]
    fn process_variables_cargo() {
        let cargo = vec![crate::models::CargoFile {
            id: "1".into(),
            url: "https://x".into(),
            target_path: "plugins/essentials.jar".into(),
            properties: Default::default(),
        }];
        let out =
            process_variables("java -jar %cargo:['plugins/essentials.jar']%", &[], Some(&cargo)).unwrap();
        assert_eq!(out, "java -jar plugins/essentials.jar");

        assert!(process_variables("%cargo:['missing.jar']%", &[], Some(&cargo)).is_err());
    }

    #[test]
    fn variable_rules() {
        assert!(validate_variable_value("", "nullable|string"));
        assert!(!validate_variable_value("abcdefgh", "max:4"));
        assert!(validate_variable_value("abc", "max:4|string"));
    }
}
