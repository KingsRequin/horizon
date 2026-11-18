//! Lecture de `git status --porcelain=v2 --branch` : branche, fichiers modifiés, commits non
//! poussés.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitInfo {
    /// `None` = tête détachée.
    pub branch: Option<String>,
    /// Fichiers modifiés, ajoutés, en conflit ou non suivis.
    pub changed: u32,
    /// Commits en avance / en retard sur la branche suivie ; `None` sans branche suivie.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

/// Analyse la sortie de `git status --porcelain=v2 --branch`.
pub fn parse_porcelain_v2(out: &str) -> GitInfo {
    let mut info = GitInfo { branch: None, changed: 0, ahead: None, behind: None };
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            info.branch = (head != "(detached)").then(|| head.to_owned());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            info.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok());
            info.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if ["1 ", "2 ", "u ", "? "].iter().any(|p| line.starts_with(p)) {
            info.changed += 1;
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_with_upstream_and_changes() {
        let out = "# branch.oid 1eca03b0000000000000000000000000000000000\n\
                   # branch.head v3-lot4\n\
                   # branch.upstream origin/v3-lot4\n\
                   # branch.ab +2 -1\n\
                   1 .M N... 100644 100644 100644 aaa bbb app/src/App.svelte\n\
                   2 R. N... 100644 100644 100644 aaa bbb R100 nouveau.rs\tancien.rs\n\
                   u UU N... 100644 100644 100644 100644 a b c conflit.txt\n\
                   ? brouillon.txt\n\
                   ! ignore.log\n";
        let g = parse_porcelain_v2(out);
        assert_eq!(g, GitInfo { branch: Some("v3-lot4".into()), changed: 4, ahead: Some(2), behind: Some(1) });
    }

    #[test]
    fn no_upstream_means_unknown_ahead() {
        let g = parse_porcelain_v2("# branch.oid abc\n# branch.head master\n");
        assert_eq!(g, GitInfo { branch: Some("master".into()), changed: 0, ahead: None, behind: None });
    }

    #[test]
    fn detached_and_initial() {
        assert_eq!(parse_porcelain_v2("# branch.oid abc\n# branch.head (detached)\n").branch, None);
        let g = parse_porcelain_v2("# branch.oid (initial)\n# branch.head main\n? a\n");
        assert_eq!((g.branch.as_deref(), g.changed), (Some("main"), 1));
    }

    #[test]
    fn windows_line_endings() {
        let g = parse_porcelain_v2("# branch.head dev\r\n# branch.ab +0 -3\r\n1 M. x\r\n");
        assert_eq!(g, GitInfo { branch: Some("dev".into()), changed: 1, ahead: Some(0), behind: Some(3) });
    }
}
