//! Lecture de `~/.ssh/config` : hôtes déclarés (blocs `Host` sans joker) et les seules valeurs
//! utiles (`HostName`, `Port`, `User`).

use serde::Serialize;

/// Hôte déclaré dans `~/.ssh/config` (bloc `Host` sans joker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
}

/// Bloc `Host` (ou `Match`, jamais appliqué) et les valeurs qu'il donne.
#[derive(Default)]
struct Block {
    patterns: Vec<String>,
    /// Bloc `Match` : conditions non évaluées, ses valeurs ne servent pas.
    is_match: bool,
    hostname: Option<String>,
    port: Option<u16>,
    user: Option<String>,
}

/// Mots d'une ligne de configuration, guillemets retirés.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Mot-clé (en minuscules) et arguments : `Clé valeur` ou `Clé=valeur`.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let (key, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((key.to_ascii_lowercase(), words(rest)))
}

/// Motif `ssh_config` (`*`, `?`), sans tenir compte de la casse.
pub(super) fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

impl Block {
    /// Le bloc s'applique à `alias` : un motif positif correspond et aucun motif `!` ne l'exclut.
    fn applies_to(&self, alias: &str) -> bool {
        if self.is_match {
            return false;
        }
        let mut hit = false;
        for p in &self.patterns {
            if let Some(neg) = p.strip_prefix('!') {
                if glob(neg, alias) {
                    return false;
                }
            } else if glob(p, alias) {
                hit = true;
            }
        }
        hit
    }
}

fn is_literal(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.starts_with('!') && !pattern.contains(['*', '?'])
}

/// Lit le texte d'un `ssh_config` : un hôte par nom de bloc `Host` sans joker, dans l'ordre du
/// fichier. Comme ssh, la première valeur trouvée pour un hôte l'emporte (blocs génériques
/// compris : `Host *` en tête du fichier s'applique à tous). `Include` n'est pas suivi.
pub fn parse_ssh_config(text: &str) -> Vec<SshHost> {
    // Lignes avant tout `Host` : valables pour tous les hôtes.
    let mut blocks = vec![Block { patterns: vec!["*".into()], ..Block::default() }];
    for line in text.lines() {
        let Some((key, args)) = split_line(line) else { continue };
        let first = args.first().cloned();
        match key.as_str() {
            "host" => blocks.push(Block { patterns: args, ..Block::default() }),
            "match" => blocks.push(Block { is_match: true, ..Block::default() }),
            _ => {
                let b = blocks.last_mut().expect("bloc courant");
                match key.as_str() {
                    "hostname" if b.hostname.is_none() => b.hostname = first,
                    "port" if b.port.is_none() => b.port = first.and_then(|p| p.parse().ok()),
                    "user" if b.user.is_none() => b.user = first,
                    _ => {}
                }
            }
        }
    }
    let mut aliases: Vec<String> = Vec::new();
    for b in blocks.iter().filter(|b| !b.is_match) {
        for p in b.patterns.iter().filter(|p| is_literal(p)) {
            if !aliases.iter().any(|a| a.eq_ignore_ascii_case(p)) {
                aliases.push(p.clone());
            }
        }
    }
    aliases
        .into_iter()
        .map(|alias| {
            let mut h = SshHost { alias: alias.clone(), hostname: None, port: None, user: None };
            for b in blocks.iter().filter(|b| b.applies_to(&alias)) {
                h.hostname = h.hostname.or_else(|| b.hostname.clone());
                h.port = h.port.or(b.port);
                h.user = h.user.or_else(|| b.user.clone());
            }
            // `%h` = le nom demandé (ici l'alias).
            h.hostname = h.hostname.map(|n| n.replace("%h", &alias));
            h
        })
        .collect()
}
