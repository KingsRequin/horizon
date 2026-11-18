//! `horizon.exe [--quit] [--essai] [--notif-probe]` : `--quit` ferme l'instance en cours ;
//! `--essai` ouvre aussi les fenêtres d'essai de la plateforme (saisie, clavier, souris) ;
//! `--notif-probe` affiche les notifications lues pendant 60 s, sans lancer Horizon.

#![windows_subsystem = "windows"]

fn main() {
    if std::env::args().any(|a| a == "--notif-probe") {
        return horizon_natif::notif_probe();
    }
    horizon_natif::run();
}
