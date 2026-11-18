//! Noms lisibles des écrans Windows (repris de `apps/prisme/lib/screens.ts`).

use super::types::Screen;

/// `\\.\DISPLAY2` → « Écran 2 », avec « principal » ou « portrait » quand c'est utile.
pub fn label(name: Option<&str>, screens: &[Screen]) -> String {
    let Some(name) = name.filter(|n| !n.is_empty()) else { return "Écran principal".into() };
    let upper = name.to_ascii_uppercase();
    let digits: String = upper.find("DISPLAY").map_or_else(String::new, |i| {
        upper[i + "DISPLAY".len()..].chars().take_while(char::is_ascii_digit).collect()
    });
    let base = if digits.is_empty() { name.to_owned() } else { format!("Écran {digits}") };
    let Some(s) = screens.iter().find(|s| s.name == name) else { return base };
    let tags: Vec<&str> = [(s.primary, "principal"), (s.h > s.w, "portrait")].into_iter().filter(|t| t.0).map(|t| t.1).collect();
    if tags.is_empty() { base } else { format!("{base} ({})", tags.join(", ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_are_named() {
        let s = vec![
            Screen { name: r"\\.\DISPLAY1".into(), w: 1.0, h: 0.56, primary: true, ..Screen::default() },
            Screen { name: r"\\.\DISPLAY3".into(), w: 0.56, h: 1.0, ..Screen::default() },
        ];
        assert_eq!(label(None, &s), "Écran principal");
        assert_eq!(label(Some(r"\\.\DISPLAY1"), &s), "Écran 1 (principal)");
        assert_eq!(label(Some(r"\\.\DISPLAY3"), &s), "Écran 3 (portrait)");
        assert_eq!(label(Some(r"\\.\DISPLAY12"), &[]), "Écran 12");
        assert_eq!(label(Some("Autre"), &[]), "Autre");
    }
}
