//! Textes affichés communs (lib/format.ts, lib/time.ts) : nom lisible d'une appli, durées en
//! horloge.

/// Applis reconnues : motif (minuscules) → nom.
const KNOWN: &[(&[&str], &str)] =
    &[(&["youtube-music", "pear"], "Pear Desktop"), (&["discord"], "Discord"), (&["spotify"], "Spotify")];

/// Empreinte ajoutée à certains AUMID : 16 caractères ou plus, majuscules et chiffres.
fn is_hash(s: &str) -> bool {
    s.len() >= 16 && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Nom lisible d'une appli à partir de son identifiant (AUMID, exe ou chemin).
pub fn app_name(id: &str) -> String {
    if id.is_empty() {
        return String::new();
    }
    let lower = id.to_lowercase();
    if let Some((_, name)) = KNOWN.iter().find(|(pats, _)| pats.iter().any(|p| lower.contains(p))) {
        return (*name).to_owned();
    }
    let last = id.rsplit(['!', '\\', '/']).next().unwrap_or(id);
    let cut = last.len().saturating_sub(4);
    let last = if last.is_char_boundary(cut) && last[cut..].eq_ignore_ascii_case(".exe") { &last[..cut] } else { last };
    // Dernier segment lisible : les navigateurs Chromium (Vivaldi, Chrome, Edge…) suffixent leur
    // AUMID d'une empreinte en base 32 (`Vivaldi.MMK7OEAQ…`) qu'on ne doit pas afficher.
    let parts: Vec<&str> = last.split('.').collect();
    let base = parts.iter().rev().find(|p| !p.is_empty() && !is_hash(p)).or(parts.last()).copied().unwrap_or(last);
    let mut c = base.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Position ou durée en horloge : 65 400 ms → « 1:05 » ; 3 723 000 ms → « 1:02:03 ».
pub fn clock(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_are_readable() {
        assert_eq!(app_name("vivaldi.exe"), "Vivaldi");
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("C:\\Apps\\YouTube Music.exe"), "YouTube Music");
        assert_eq!(app_name("Vivaldi.MMK7OEAQLRVYQQEXZF7V45UTJE"), "Vivaldi");
        assert_eq!(app_name("Chrome.UJVRCRRVQ6A6SYMKRBE3ISPC4A"), "Chrome");
        assert_eq!(app_name("com.github.th-ch.youtube-music"), "Pear Desktop");
        assert_eq!(app_name("com.squirrel.Discord.Discord"), "Discord");
        assert_eq!(app_name(""), "");
    }

    #[test]
    fn clock_text() {
        assert_eq!(clock(65_400), "1:05");
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(clock(0), "0:00");
    }
}
