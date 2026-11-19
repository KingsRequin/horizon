//! `horizon.exe [--quit] [--essai] [--notif-probe]` : `--quit` ferme l'instance en cours ;
//! `--essai` ouvre aussi les fenêtres d'essai de la plateforme (saisie, clavier, souris) ;
//! `--notif-probe` affiche les notifications lues pendant 60 s, sans lancer Horizon ;
//! `--check-plugin <dossier>` vérifie un plugin (code de sortie 0 : il se chargera) ;
//! `--install-plugin <id>` l'installe depuis la bibliothèque.

#![windows_subsystem = "windows"]

fn main() {
    if std::env::args().any(|a| a == "--notif-probe") {
        return horizon_natif::notif_probe();
    }
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--install-plugin") {
        horizon_natif::attach_console();
        let code = match args.get(i + 1) {
            Some(id) => horizon_natif::plugins::library::cli(id),
            None => {
                println!("usage : horizon.exe --install-plugin <identifiant du plugin>");
                2
            }
        };
        std::process::exit(code);
    }
    if let Some(i) = args.iter().position(|a| a == "--check-plugin") {
        horizon_natif::attach_console();
        let code = match args.get(i + 1) {
            Some(dir) => horizon_natif::plugins::check::cli(std::path::Path::new(dir)),
            None => {
                println!("usage : horizon.exe --check-plugin <dossier du plugin>");
                2
            }
        };
        std::process::exit(code);
    }
    horizon_natif::run();
}
