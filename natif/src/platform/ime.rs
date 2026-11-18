//! Méthode de saisie (IME) : placement de la fenêtre de composition et de la liste de candidats
//! au curseur de texte du champ actif. Le texte validé arrive par `WM_CHAR` (`DefWindowProc`
//! traduit le résultat de la composition), comme les touches mortes et le panneau des emoji.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_EXCLUDE, CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
    ImmSetCandidateWindow, ImmSetCompositionWindow,
};

/// Fenêtre de composition au point (`x`, `top`) de la zone cliente ; candidats sous le curseur
/// (`bottom`), sans le recouvrir.
pub fn place(hwnd: HWND, x: i32, top: i32, bottom: i32) {
    let (composition, candidate) = forms(x, top, bottom);
    // SAFETY : contexte de la fenêtre de ce fil, rendu juste après ; structures locales.
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        let _ = ImmSetCompositionWindow(himc, &composition);
        let _ = ImmSetCandidateWindow(himc, &candidate);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

fn forms(x: i32, top: i32, bottom: i32) -> (COMPOSITIONFORM, CANDIDATEFORM) {
    let composition = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x, y: top }, ..Default::default() };
    let candidate = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: CFS_CANDIDATEPOS | CFS_EXCLUDE,
        ptCurrentPos: POINT { x, y: bottom },
        rcArea: RECT { left: x, top, right: x + 1, bottom },
    };
    (composition, candidate)
}

#[cfg(test)]
mod tests {
    use super::forms;

    #[test]
    fn candidats_sous_le_curseur_sans_le_couvrir() {
        let (c, k) = forms(40, 10, 28);
        assert_eq!((c.ptCurrentPos.x, c.ptCurrentPos.y), (40, 10));
        assert_eq!((k.ptCurrentPos.x, k.ptCurrentPos.y), (40, 28));
        assert_eq!((k.rcArea.top, k.rcArea.bottom), (10, 28), "zone exclue = hauteur du curseur");
    }
}
