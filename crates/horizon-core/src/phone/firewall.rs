//! Pare-feu de Windows : Horizon peut-il recevoir un téléphone ? Décision pure, à partir des
//! règles lues par `horizon_sys::firewall`, et commande qui pose la règle d'Horizon.
//!
//! Windows crée ses règles par chemin d'exécutable, et une demande d'autorisation restée sans
//! réponse laisse une règle de blocage qui l'emporte sur toute autorisation : d'où une règle
//! unique, posée par Horizon lui-même, et un diagnostic qui repère les blocages.

/// Nom de la règle d'Horizon dans le pare-feu de Windows (sans accent : il traverse `cmd`).
pub const RULE_NAME: &str = "Horizon Mobile";
/// Premier port d'écoute ; s'il est pris, les suivants, [`PORT_TRIES`] en tout.
pub const PORT: u16 = 28680;
pub const PORT_TRIES: u16 = 10;

/// Ce que le pare-feu de Windows fera d'une connexion entrante vers Horizon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallState {
    /// Une règle l'autorise et aucune ne la bloque.
    Allowed,
    /// Aucune règle : Windows la refuserait (et demanderait à l'utilisateur à la première écoute).
    Missing,
    /// Une règle la bloque, quelles que soient les autorisations.
    Blocked,
    /// Règles illisibles, ou pare-feu de Windows coupé (un autre pare-feu décide).
    Unknown,
}

/// Une règle du pare-feu, réduite à ce qui compte ici.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleView {
    /// Programme visé ; vide = tous.
    pub program: String,
    /// Ports locaux (« 80,443 », « 1000-2000 », « * » ou vide = tous).
    pub ports: String,
    pub allow: bool,
    pub enabled: bool,
    pub inbound: bool,
    /// Protocole TCP, ou tous.
    pub tcp: bool,
    /// S'applique au profil de réseau privé.
    pub private: bool,
}

/// `ports` (liste de ports et de plages) couvre-t-il `port` ?
pub fn covers(ports: &str, port: u16) -> bool {
    let ports = ports.trim();
    if ports.is_empty() || ports == "*" {
        return true;
    }
    ports.split(',').any(|part| match part.trim().split_once('-') {
        Some((a, b)) => matches!((a.trim().parse::<u16>(), b.trim().parse::<u16>()), (Ok(a), Ok(b)) if a <= port && port <= b),
        None => part.trim().parse::<u16>() == Ok(port),
    })
}

/// La règle vise-t-elle `exe` ? Une règle sans programme vaut pour tous, mais seulement si
/// elle nomme ses ports : sans programme ni port, c'est la règle d'un service ou d'une appli
/// empaquetée, que Windows restreint par d'autres champs.
fn targets(rule: &RuleView, exe: &str) -> bool {
    if rule.program.is_empty() {
        let ports = rule.ports.trim();
        return !ports.is_empty() && ports != "*";
    }
    rule.program.to_lowercase() == exe.to_lowercase()
}

/// La règle concerne-t-elle une connexion TCP entrante vers `exe` sur `port`, en réseau privé ?
fn applies(rule: &RuleView, exe: &str, port: u16) -> bool {
    rule.enabled
        && rule.inbound
        && rule.tcp
        && rule.private
        && targets(rule, exe)
        && covers(&rule.ports, port)
}

/// État du pare-feu pour `exe` écoutant sur `port` : un blocage l'emporte sur une autorisation.
pub fn decide(rules: &[RuleView], exe: &str, port: u16) -> FirewallState {
    let mut state = FirewallState::Missing;
    for rule in rules.iter().filter(|r| applies(r, exe, port)) {
        if !rule.allow {
            return FirewallState::Blocked;
        }
        state = FirewallState::Allowed;
    }
    state
}

/// Commandes (à lancer en administrateur, par `cmd /s /c`) qui remettent les règles de `exe`
/// en ordre : toutes ses règles entrantes sont retirées, blocages compris, puis la règle
/// d'Horizon est posée pour ses ports, en réseau privé seulement.
pub fn netsh_script(exe: &str) -> String {
    let last = PORT + PORT_TRIES - 1;
    format!(
        r#"netsh advfirewall firewall delete rule name=all dir=in program="{exe}" & netsh advfirewall firewall add rule name="{RULE_NAME}" dir=in action=allow protocol=TCP localport={PORT}-{last} profile=private program="{exe}""#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\x\AppData\Local\Programs\Horizon\horizon.exe";

    fn allow() -> RuleView {
        RuleView { program: EXE.to_uppercase(), ports: "28680-28689".into(), allow: true, enabled: true, inbound: true, tcp: true, private: true }
    }

    #[test]
    fn a_block_rule_wins_over_an_allow_rule() {
        assert_eq!(decide(&[], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[allow()], EXE, 28680), FirewallState::Allowed);
        assert_eq!(decide(&[allow()], EXE, 28689), FirewallState::Allowed);
        assert_eq!(decide(&[RuleView { ports: "47800".into(), ..allow() }], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[RuleView { private: false, ..allow() }], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[RuleView { enabled: false, ..allow() }], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[RuleView { tcp: false, ..allow() }], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[RuleView { inbound: false, ..allow() }], EXE, 28680), FirewallState::Missing);
        assert_eq!(decide(&[allow(), RuleView { allow: false, ports: "*".into(), ..allow() }], EXE, 28680), FirewallState::Blocked);
        assert_eq!(decide(&[RuleView { program: r"C:\autre.exe".into(), allow: false, ..allow() }], EXE, 28680), FirewallState::Missing);
    }

    #[test]
    fn rules_that_cannot_stop_us_are_ignored() {
        let block = RuleView { allow: false, ports: String::new(), ..allow() };
        assert_eq!(decide(&[allow(), RuleView { tcp: false, ..block.clone() }], EXE, 28680), FirewallState::Allowed, "blocage UDP seulement");
        assert_eq!(decide(&[allow(), RuleView { private: false, ..block.clone() }], EXE, 28680), FirewallState::Allowed, "blocage en réseau public seulement");
        assert_eq!(decide(&[allow(), RuleView { enabled: false, ..block.clone() }], EXE, 28680), FirewallState::Allowed);
        assert_eq!(decide(&[allow(), RuleView { ports: "80,443".into(), ..block }], EXE, 28680), FirewallState::Allowed, "blocage d'autres ports");
    }

    #[test]
    fn a_rule_for_every_program_counts_too() {
        let any = RuleView { program: String::new(), ports: "28000-29000".into(), ..allow() };
        assert_eq!(decide(std::slice::from_ref(&any), EXE, 28680), FirewallState::Allowed);
        let vague = RuleView { ports: String::new(), ..any.clone() };
        assert_eq!(decide(std::slice::from_ref(&vague), EXE, 28680), FirewallState::Missing, "sans programme ni port : règle d'un service ou d'une appli empaquetée, pas la nôtre");
        assert_eq!(decide(&[allow(), RuleView { allow: false, ..any }], EXE, 28680), FirewallState::Blocked);
    }

    #[test]
    fn port_lists_and_ranges_are_read() {
        for (ports, port, expected) in [("", 5, true), ("*", 5, true), ("28680", 28680, true), ("80, 28680 ,443", 28680, true), ("28680-28689", 28685, true), ("28680-28689", 28690, false), ("abc", 1, false), ("90-80", 85, false)] {
            assert_eq!(covers(ports, port), expected, "« {ports} » et {port}");
        }
    }

    #[test]
    fn the_script_replaces_every_rule_of_the_program() {
        let s = netsh_script(r"C:\P F\horizon.exe");
        assert_eq!(
            s,
            r#"netsh advfirewall firewall delete rule name=all dir=in program="C:\P F\horizon.exe" & netsh advfirewall firewall add rule name="Horizon Mobile" dir=in action=allow protocol=TCP localport=28680-28689 profile=private program="C:\P F\horizon.exe""#
        );
    }
}
