//! Consommation des sessions Claude Code, lue dans leurs transcriptions
//! (`~/.claude/projects/<projet>/<session>.jsonl`, et `<session>/subagents/*.jsonl` pour les
//! sous-agents). Le format n'est pas documenté : seules les lignes `"type":"assistant"` qui
//! portent `message.usage` comptent ; une ligne illisible ou d'une autre forme est ignorée.
//!
//! Lecture incrémentale : chaque fichier est relu depuis la position atteinte (une ligne
//! incomplète attend la suite) ; un fichier raccourci ou remplacé est relu depuis le début.
//! Claude Code écrit une ligne par bloc de contenu d'une réponse, avec le même `usage` : un
//! même `message.id` ne compte qu'une fois. Les jetons sont rangés par heure (fin de journée
//! locale alignée sur l'heure), les heures de plus de deux jours oubliées.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const HOUR_MS: u64 = 3_600_000;
/// Heures gardées (un peu plus d'une journée locale, quel que soit le fuseau).
const KEEP_MS: u64 = 48 * HOUR_MS;
/// Lu au plus par fichier et par relecture : une très longue transcription est rattrapée en
/// plusieurs fois au lieu de bloquer le fil.
const MAX_READ: u64 = 32 * 1024 * 1024;
/// Une ligne plus longue est sautée (sortie d'outil énorme) : jamais gardée en mémoire.
const MAX_LINE: usize = 4 * 1024 * 1024;
/// Identifiants de messages déjà comptés, gardés (les doublons se suivent).
const RECENT_IDS: usize = 256;
/// Fenêtre de contexte supposée ; une session qui la dépasse en a une plus grande.
pub const CONTEXT_SMALL: u64 = 200_000;
pub const CONTEXT_LARGE: u64 = 1_000_000;
/// Transcription muette depuis ce délai, sans session suivie : oubliée.
const FORGET_MS: u64 = 36 * HOUR_MS;

/// Jetons d'une ou plusieurs réponses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Écrits dans le cache (facturés comme l'entrée, un peu plus).
    pub cache_write: u64,
    /// Relus du cache (bien moins chers) : comptés à part.
    pub cache_read: u64,
}

impl Tokens {
    /// Jetons « neufs » : entrée, écriture du cache et sortie (sans les relectures du cache).
    pub fn fresh(&self) -> u64 {
        self.input + self.cache_write + self.output
    }

    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
    }
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    #[serde(rename = "requestId", default)]
    request_id: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
}

/// Une réponse lue : instant, jetons, et taille du contexte (fil principal seulement).
#[derive(Debug, PartialEq)]
struct Entry {
    at_ms: u64,
    tokens: Tokens,
    context: Option<u64>,
    large: bool,
}

/// Lit une ligne ; `None` si ce n'est pas une réponse avec consommation.
fn parse_line(line: &[u8], seen: &mut VecDeque<String>) -> Option<Entry> {
    // Filtre rapide : la plupart des lignes (outils, messages de l'utilisateur) n'ont pas d'usage.
    if !contains(line, b"\"usage\"") || !contains(line, b"\"assistant\"") {
        return None;
    }
    let raw: RawLine = serde_json::from_slice(line).ok()?;
    if raw.kind.as_deref() != Some("assistant") {
        return None;
    }
    let msg = raw.message?;
    let u = msg.usage?;
    let key = msg.id.or(raw.request_id);
    if let Some(k) = &key {
        if seen.contains(k) {
            return None;
        }
        if seen.len() >= RECENT_IDS {
            seen.pop_front();
        }
        seen.push_back(k.clone());
    }
    let at_ms = chrono::DateTime::parse_from_rfc3339(raw.timestamp.as_deref()?.trim()).ok()?.timestamp_millis();
    let tokens = Tokens {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_write: u.cache_creation_input_tokens,
        cache_read: u.cache_read_input_tokens,
    };
    let prompt = tokens.input + tokens.cache_write + tokens.cache_read;
    Some(Entry {
        at_ms: u64::try_from(at_ms).ok()?,
        tokens,
        context: (!raw.sidechain).then_some(prompt + tokens.output),
        large: msg.model.is_some_and(|m| m.contains("[1m]")),
    })
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Lecture incrémentale d'une transcription.
#[derive(Debug, Default)]
pub struct Transcript {
    offset: u64,
    /// Début d'une ligne pas encore terminée.
    partial: Vec<u8>,
    /// La ligne en cours est trop longue : sautée jusqu'à sa fin.
    skipping: bool,
    seen: VecDeque<String>,
    /// Jetons par heure (début de l'heure, ms Unix).
    hours: BTreeMap<u64, Tokens>,
    /// Contexte de la réponse la plus récente du fil principal, et son instant.
    context: Option<u64>,
    context_ms: u64,
    /// Le contexte a dépassé la petite fenêtre (ou le modèle l'annonce).
    large: bool,
    /// Instant de la dernière réponse lue.
    last_ms: u64,
}

impl Transcript {
    /// Lit la suite du fichier. Vrai si quelque chose a changé. Fichier absent : rien.
    pub fn read_new(&mut self, path: &Path) -> std::io::Result<bool> {
        let mut f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let len = f.metadata()?.len();
        if len < self.offset {
            // Raccourci ou remplacé : tout est relu.
            *self = Transcript::default();
        }
        if len == self.offset {
            return Ok(false);
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.take(MAX_READ).read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        Ok(self.feed(&buf))
    }

    /// Ajoute des octets lus ; vrai si une réponse a été comptée.
    fn feed(&mut self, mut bytes: &[u8]) -> bool {
        let mut changed = false;
        while let Some(i) = bytes.iter().position(|&b| b == b'\n') {
            let (line, rest) = (&bytes[..i], &bytes[i + 1..]);
            bytes = rest;
            if std::mem::take(&mut self.skipping) {
                self.partial.clear();
                continue;
            }
            let entry = if self.partial.is_empty() {
                parse_line(line, &mut self.seen)
            } else {
                self.partial.extend_from_slice(line);
                let whole = std::mem::take(&mut self.partial);
                parse_line(&whole, &mut self.seen)
            };
            if let Some(e) = entry {
                self.add(e);
                changed = true;
            }
        }
        if !self.skipping {
            self.partial.extend_from_slice(bytes);
            if self.partial.len() > MAX_LINE {
                self.partial = Vec::new();
                self.skipping = true;
            }
        }
        changed
    }

    fn add(&mut self, e: Entry) {
        self.hours.entry(e.at_ms - e.at_ms % HOUR_MS).or_default().add(&e.tokens);
        if let Some(c) = e.context {
            if e.at_ms >= self.context_ms {
                (self.context, self.context_ms) = (Some(c), e.at_ms);
            }
            self.large |= e.large || c > CONTEXT_SMALL;
        }
        self.last_ms = self.last_ms.max(e.at_ms);
    }

    /// Jetons depuis `since_ms` (début de la journée locale).
    pub fn since(&self, since_ms: u64) -> Tokens {
        let mut t = Tokens::default();
        for v in self.hours.range(since_ms - since_ms % HOUR_MS..).map(|(_, v)| v) {
            t.add(v);
        }
        t
    }

    /// Oublie les heures trop anciennes.
    fn prune(&mut self, now_ms: u64) {
        let keep = now_ms.saturating_sub(KEEP_MS);
        self.hours.retain(|&h, _| h >= keep);
    }
}

/// Résumé lisible (diagnostic) : « 12 345 neufs (entrée 2, cache 10 000, sortie 2 343), 99 relus ».
pub fn fresh_label(t: &Tokens) -> String {
    format!(
        "{} neufs (entrée {}, cache {}, sortie {}), {} relus",
        t.fresh(),
        t.input,
        t.cache_write,
        t.output,
        t.cache_read
    )
}

/// Consommation d'une session (panneau Claude).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionUsage {
    pub id: String,
    /// Jetons du jour (sous-agents compris).
    pub today: Tokens,
    /// Contexte de la dernière réponse (jetons) et fenêtre supposée.
    pub context: Option<u64>,
    pub window: u64,
}

/// Jetons du jour d'un projet (dossier).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectUsage {
    pub name: String,
    pub today: Tokens,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageView {
    pub today: Tokens,
    /// Projets du jour, les plus gourmands d'abord.
    pub projects: Vec<ProjectUsage>,
    pub sessions: Vec<SessionUsage>,
}

/// Session suivie : sa transcription et celles de ses sous-agents.
#[derive(Debug)]
struct Tracked {
    project: String,
    main_path: PathBuf,
    main: Transcript,
    agents: HashMap<PathBuf, Transcript>,
}

/// Transcriptions des sessions vues par les hooks.
#[derive(Debug, Default)]
pub struct UsageBook {
    sessions: HashMap<String, Tracked>,
}

/// Chemin d'une transcription d'après le dossier de travail, quand le hook ne l'a pas donné
/// (Claude Code remplace chaque caractère hors lettres, chiffres et `-` par `-`).
pub fn guess_transcript(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    let dir: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    home.join(".claude").join("projects").join(dir).join(format!("{session_id}.jsonl"))
}

impl UsageBook {
    /// Session vue par un hook (dossier de travail, transcription).
    pub fn track(&mut self, id: &str, cwd: &str, path: PathBuf) {
        let project = if cwd.is_empty() { String::new() } else { crate::claude_sessions::folder_name(cwd) };
        let project = project.as_str();
        let t = self.sessions.entry(id.to_owned()).or_insert_with(|| Tracked {
            project: project.to_owned(),
            main_path: path.clone(),
            main: Transcript::default(),
            agents: HashMap::new(),
        });
        if t.main_path != path {
            t.main_path = path;
            t.main = Transcript::default();
        }
        if !project.is_empty() {
            t.project = project.to_owned();
        }
    }

    /// Relit la suite des transcriptions (sous-agents compris) ; vrai si quelque chose a changé.
    /// `active` : sessions encore suivies (les autres sont oubliées après 36 h sans réponse).
    pub fn refresh(&mut self, now_ms: u64, active: &[String]) -> bool {
        let mut changed = false;
        for t in self.sessions.values_mut() {
            changed |= read_logged(&mut t.main, &t.main_path);
            for p in agent_files(&t.main_path) {
                changed |= read_logged(t.agents.entry(p.clone()).or_default(), &p);
            }
            t.main.prune(now_ms);
            for a in t.agents.values_mut() {
                a.prune(now_ms);
            }
        }
        self.sessions.retain(|id, t| {
            let last = t.agents.values().map(|a| a.last_ms).fold(t.main.last_ms, u64::max);
            active.contains(id) || now_ms.saturating_sub(last) < FORGET_MS
        });
        changed
    }

    /// Consommation depuis `day_start_ms` (minuit, heure locale).
    pub fn view(&self, day_start_ms: u64) -> UsageView {
        let mut out = UsageView::default();
        let mut projects: BTreeMap<String, Tokens> = BTreeMap::new();
        for (id, t) in &self.sessions {
            let mut today = t.main.since(day_start_ms);
            for a in t.agents.values() {
                today.add(&a.since(day_start_ms));
            }
            out.today.add(&today);
            if today.fresh() > 0 || today.cache_read > 0 {
                projects.entry(t.project.clone()).or_default().add(&today);
            }
            let window = if t.main.large { CONTEXT_LARGE } else { CONTEXT_SMALL };
            out.sessions.push(SessionUsage { id: id.clone(), today, context: t.main.context, window });
        }
        out.projects = projects.into_iter().map(|(name, today)| ProjectUsage { name, today }).collect();
        out.projects.sort_by(|a, b| b.today.fresh().cmp(&a.today.fresh()).then(a.name.cmp(&b.name)));
        out.sessions.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

fn read_logged(t: &mut Transcript, p: &Path) -> bool {
    t.read_new(p).unwrap_or_else(|e| {
        log::debug!("transcription {} : {e}", p.display());
        false
    })
}

/// Transcriptions des sous-agents d'une session : `<dossier>/<session>/subagents/*.jsonl`.
fn agent_files(main: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (main.parent(), main.file_stem()) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir.join(stem).join("subagents")) else { return Vec::new() };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::io::Write;

    fn line(id: &str, at: &str, input: u64, write: u64, read: u64, out: u64, side: bool) -> String {
        format!(
            r#"{{"parentUuid":"x","isSidechain":{side},"type":"assistant","timestamp":"{at}","requestId":"r{id}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":{input},"cache_creation_input_tokens":{write},"cache_read_input_tokens":{read},"output_tokens":{out},"server_tool_use":{{}}}},"content":[]}}}}"#
        ) + "\n"
    }

    const DAY: &str = "2026-11-12T";
    fn ms(hhmm: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(&format!("{DAY}{hhmm}:00Z")).unwrap().timestamp_millis() as u64
    }

    fn append(p: &Path, s: &str) {
        std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(s.as_bytes()).unwrap();
    }

    #[test]
    fn duplicates_and_other_lines_are_ignored() {
        let mut t = Transcript::default();
        let mut text = String::new();
        text += r#"{"type":"user","message":{"role":"user","content":"usage assistant"}}"# ;
        text += "\n";
        text += &line("m1", "2026-11-12T10:00:00Z", 2, 100, 1000, 50, false);
        // Même message, autre bloc de contenu : même usage, compté une fois.
        text += &line("m1", "2026-11-12T10:00:01Z", 2, 100, 1000, 50, false);
        text += "pas du json mais \"usage\" \"assistant\"\n";
        text += &line("m2", "2026-11-12T11:30:00Z", 1, 10, 1200, 20, false);
        assert!(t.feed(text.as_bytes()));
        let all = t.since(0);
        assert_eq!(all, Tokens { input: 3, output: 70, cache_write: 110, cache_read: 2200 });
        assert_eq!(all.fresh(), 183);
        assert_eq!(t.context, Some(1 + 10 + 1200 + 20));
        // Depuis 11 h : seule la seconde réponse.
        assert_eq!(t.since(ms("11:00")).fresh(), 31);
    }

    #[test]
    fn partial_lines_wait_for_their_end() {
        let mut t = Transcript::default();
        let l = line("m1", "2026-11-12T10:00:00Z", 5, 0, 0, 5, false);
        let (a, b) = l.split_at(40);
        assert!(!t.feed(a.as_bytes()));
        assert!(t.feed(b.as_bytes()));
        assert_eq!(t.since(0).fresh(), 10);
    }

    #[test]
    fn sidechain_counts_tokens_but_not_context_and_large_context_is_detected() {
        let mut t = Transcript::default();
        t.feed(line("a", "2026-11-12T10:00:00Z", 0, 0, 250_000, 10, false).as_bytes());
        t.feed(line("b", "2026-11-12T10:01:00Z", 0, 0, 9, 1, true).as_bytes());
        assert_eq!(t.context, Some(250_010));
        assert!(t.large);
    }

    #[test]
    fn reading_is_incremental_and_restarts_on_a_rewritten_file() {
        let d = TestDir::new("usage-incr");
        let p = d.join("s.jsonl");
        let mut t = Transcript::default();
        assert!(!t.read_new(&p).unwrap(), "fichier absent : rien");
        append(&p, &line("m1", "2026-11-12T10:00:00Z", 1, 0, 0, 1, false));
        assert!(t.read_new(&p).unwrap());
        assert!(!t.read_new(&p).unwrap(), "rien de nouveau");
        append(&p, &line("m2", "2026-11-12T10:05:00Z", 2, 0, 0, 2, false));
        assert!(t.read_new(&p).unwrap());
        assert_eq!(t.since(0).fresh(), 6);
        // Fichier remplacé par un plus court : relu depuis le début.
        std::fs::write(&p, line("m9", "2026-11-12T12:00:00Z", 1, 0, 0, 0, false)).unwrap();
        assert!(t.read_new(&p).unwrap());
        assert_eq!(t.since(0).fresh(), 1);
    }

    #[test]
    fn overlong_lines_are_skipped_without_losing_the_next() {
        let mut t = Transcript::default();
        let huge = vec![b'x'; MAX_LINE + 10];
        t.feed(&huge);
        assert!(t.partial.is_empty() && t.skipping);
        t.feed(b"fin de la ligne enorme\n");
        assert!(t.feed(line("m1", "2026-11-12T10:00:00Z", 1, 0, 0, 1, false).as_bytes()));
        assert_eq!(t.since(0).fresh(), 2);
    }

    #[test]
    fn book_sums_sessions_agents_and_projects() {
        let d = TestDir::new("usage-book");
        let main = d.join("s1.jsonl");
        append(&main, &line("a", "2026-11-12T09:00:00Z", 10, 0, 0, 10, false));
        // La veille : pas dans la journée.
        append(&main, &line("y", "2026-11-11T22:00:00Z", 999, 0, 0, 0, false));
        std::fs::create_dir_all(d.join("s1").join("subagents")).unwrap();
        append(&d.join("s1").join("subagents").join("agent-1.jsonl"), &line("g", "2026-11-12T09:10:00Z", 5, 0, 0, 5, true));
        let other = d.join("s2.jsonl");
        append(&other, &line("b", "2026-11-12T09:20:00Z", 1, 0, 0, 1, false));
        let mut book = UsageBook::default();
        book.track("s1", r"E:\Documents\deskbar", main.clone());
        book.track("s2", "/home/moi/rgb-engine", other);
        let now = ms("12:00");
        assert!(book.refresh(now, &["s1".into(), "s2".into()]));
        let v = book.view(ms("00:00"));
        assert_eq!(v.today.fresh(), 32);
        assert_eq!(v.projects.iter().map(|p| (p.name.as_str(), p.today.fresh())).collect::<Vec<_>>(), [("deskbar", 30), ("rgb-engine", 2)]);
        let s1 = v.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!((s1.today.fresh(), s1.context, s1.window), (30, Some(20), CONTEXT_SMALL));
        assert!(!book.refresh(now, &[]), "rien de nouveau");
        // Sans session suivie et muettes depuis longtemps : oubliées.
        book.refresh(now + FORGET_MS + HOUR_MS, &[]);
        assert!(book.view(0).sessions.is_empty());
    }

    #[test]
    fn transcript_path_is_guessed_from_the_folder() {
        let p = guess_transcript(Path::new("C:/Users/moi"), r"E:\Documents\APPLICATION\SharkApp\kings-hub", "abc");
        assert!(p.ends_with(Path::new(".claude/projects/E--Documents-APPLICATION-SharkApp-kings-hub/abc.jsonl")));
    }
}
