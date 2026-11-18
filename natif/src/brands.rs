//! Logos Simple Icons (paquet `simple-icons` 16.37, licence CC0 : `icons/simple-icons/`), embarqués
//! dans l'exe : un logo par ligne de `brands.tsv` (`slug`, couleur, nom, tracé SVG 24 × 24),
//! lignes triées par identifiant. Comme `lib/brand.ts` de la version Tauri, qui chargeait
//! `brands/<slug>.json` ; ici une recherche dichotomique dans le texte embarqué (seules les
//! pages lues entrent en mémoire).
//!
//! Le texte ne se régénère qu'à une mise à jour du paquet (commande `node` dans `natif/README.md`,
//! « Thème, polices, icônes ») ; la recherche par nom des réglages des raccourcis (`search`)
//! parcourt le même fichier.

const DATA: &str = include_str!("../icons/simple-icons/brands.tsv");

/// Logo d'une marque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brand {
    pub slug: &'static str,
    /// Couleur de marque, `RRGGBB` sans `#`.
    pub hex: &'static str,
    pub title: &'static str,
    /// Tracé SVG (boîte 0 0 24 24).
    pub path: &'static str,
}

fn parse(line: &'static str) -> Option<Brand> {
    let mut f = line.splitn(4, '\t');
    Some(Brand { slug: f.next()?, hex: f.next()?, title: f.next()?, path: f.next()? })
}

/// Logo d'identifiant `slug` (`youtube`, `discord`…) ; `None` s'il n'existe pas ou si
/// l'identifiant n'a pas la forme d'un identifiant Simple Icons.
pub fn brand(slug: &str) -> Option<Brand> {
    if slug.is_empty() || !slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return None;
    }
    let (mut lo, mut hi) = (0, DATA.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let start = DATA[..mid].rfind('\n').map_or(0, |i| i + 1);
        let end = DATA[start..].find('\n').map_or(DATA.len(), |i| start + i);
        let line = &DATA[start..end];
        let key = line.split('\t').next().unwrap_or("");
        match key.cmp(slug) {
            std::cmp::Ordering::Equal => return parse(line),
            std::cmp::Ordering::Less => lo = end + 1,
            std::cmp::Ordering::Greater => hi = start,
        }
    }
    None
}

/// Recherche par nom ou identifiant (`searchBrands` de lib/brand.ts) : les correspondances en
/// début de nom d'abord, `limit` au plus ; rien pour une recherche vide.
pub fn search(query: &str, limit: usize) -> Vec<Brand> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let all: Vec<Brand> = DATA.lines().filter_map(parse).collect();
    let starts = |b: &Brand| b.title.to_lowercase().starts_with(&q) || b.slug.starts_with(&q);
    let contains = |b: &Brand| b.title.to_lowercase().contains(&q) || b.slug.contains(&q);
    let mut out: Vec<Brand> = all.iter().filter(|b| starts(b)).copied().collect();
    out.extend(all.iter().filter(|b| !starts(b) && contains(b)).copied());
    out.truncate(limit);
    out
}

/// Logo de l'appli d'une notification d'après son nom (« Discord », « YouTube Music »…) : nom en
/// minuscules sans ce qui n'est ni lettre ni chiffre (`lib/AppMark.svelte`).
pub fn by_app_name(name: &str) -> Option<Brand> {
    let slug: String = name.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect();
    brand(&slug)
}

/// Luminance relative (WCAG) d'une couleur `RRGGBB` (avec ou sans `#`), de 0 à 1 ; `None` si
/// elle est invalide.
pub fn luminance(hex: &str) -> Option<f32> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    let lin = |c: u32| {
        let v = c as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    Some(0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255))
}

/// Couleur de marque si elle se voit sur fond sombre (`lib/color.ts`, `readableOnDark`) ; `None`
/// = la couleur du texte de la barre.
pub fn readable_on_dark(hex: &str) -> Option<slint::Color> {
    if luminance(hex)? < 0.06 {
        return None;
    }
    parse_color(hex)
}

/// Couleur `#RRGGBB` (ou `RRGGBB`) ; `None` si invalide.
pub fn parse_color(hex: &str) -> Option<slint::Color> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(slint::Color::from_rgb_u8((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Logo en SVG blanc (coloré à l'affichage par `colorize`).
pub fn svg(b: &Brand) -> String {
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#fff" d="{}"/></svg>"##, b.path)
}

/// Image du logo (fil de l'interface : `slint::Image` n'est pas envoyable).
pub fn image(b: &Brand) -> slint::Image {
    slint::Image::load_from_svg_data(svg(b).as_bytes()).unwrap_or_else(|e| {
        log::warn!("logo « {} » illisible : {e}", b.slug);
        slint::Image::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_finds_every_logo_and_only_them() {
        let mut n = 0;
        for line in DATA.lines() {
            let slug = line.split('\t').next().unwrap();
            assert_eq!(brand(slug).map(|b| b.slug), Some(slug), "{slug}");
            n += 1;
        }
        assert!(n > 3000);
        assert_eq!(brand("youtube").unwrap().hex, "FF0000");
        assert_eq!(brand("discord").unwrap().title, "Discord");
        assert!(brand("inexistant_zz").is_none());
        assert!(brand("").is_none());
        assert!(brand("../x").is_none());
        assert!(brand("YouTube").is_none(), "identifiant en minuscules");
    }

    #[test]
    fn lines_are_sorted_for_the_dichotomy() {
        let slugs: Vec<&str> = DATA.lines().map(|l| l.split('\t').next().unwrap()).collect();
        assert!(slugs.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn search_puts_the_names_that_start_with_the_query_first() {
        let r = search("YouTube", 48);
        assert_eq!(r.first().map(|b| b.slug), Some("youtube"));
        assert!(r.iter().all(|b| b.title.to_lowercase().contains("youtube") || b.slug.contains("youtube")));
        let steam = search("steam", 48);
        let first_other = steam.iter().position(|b| !b.title.to_lowercase().starts_with("steam") && !b.slug.starts_with("steam"));
        let last_start = steam.iter().rposition(|b| b.title.to_lowercase().starts_with("steam") || b.slug.starts_with("steam"));
        assert!(first_other.is_none_or(|o| last_start.is_none_or(|l| l < o)), "débuts d'abord");
        assert_eq!(search("e", 5).len(), 5);
        assert!(search("  ", 48).is_empty());
    }

    #[test]
    fn app_names_become_slugs() {
        assert_eq!(by_app_name("Discord").map(|b| b.slug), Some("discord"));
        assert_eq!(by_app_name("YouTube Music").map(|b| b.slug), Some("youtubemusic"));
        assert!(by_app_name("Courrier !").is_none());
    }

    #[test]
    fn dark_brands_take_the_text_color() {
        assert!(readable_on_dark("000000").is_none());
        assert!(readable_on_dark("181717").is_none(), "GitHub, presque noir");
        assert_eq!(readable_on_dark("#FF0000"), Some(slint::Color::from_rgb_u8(255, 0, 0)));
        assert!(readable_on_dark("zz").is_none());
        assert!(luminance("FFFFFF").unwrap() > 0.99);
    }

    #[test]
    fn logo_renders() {
        let img = image(&brand("youtube").unwrap());
        assert!(img.size().width > 0);
    }
}
