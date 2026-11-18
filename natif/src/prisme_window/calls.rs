//! Requêtes au moteur (`call` de `daemon.svelte.ts`) : numéro, réponse attendue, échec de toutes
//! les requêtes en cours quand la connexion tombe. Logique pure (l'envoi passe par `Outbox`), sans
//! fil.

use serde_json::{Value, json};
use std::collections::HashMap;

/// Où partent les messages (la connexion, ou un faux système dans les essais).
pub trait Outbox {
    fn send(&self, msg: String);
}

/// Suite d'une requête : la réponse du moteur (`data`) ou son erreur.
pub type Reply = Box<dyn FnOnce(Result<Value, String>)>;

/// Réponse rendue sans passer par le moteur : à appeler hors de tout emprunt (asynchrone, comme
/// une promesse).
pub struct Immediate(pub Reply, pub Result<Value, String>);

impl Immediate {
    pub fn run(self) {
        (self.0)(self.1);
    }
}

pub struct Calls {
    next_id: u64,
    pending: HashMap<u64, Reply>,
}

impl Calls {
    pub fn new() -> Calls {
        Calls { next_id: 1, pending: HashMap::new() }
    }

    /// Envoie `cmd` (et ses arguments, un objet) ; `reply` recevra la réponse. Hors connexion,
    /// l'échec est rendu tout de suite (`Immediate`, à exécuter par l'appelant).
    pub fn call(&mut self, out: &dyn Outbox, connected: bool, cmd: &str, args: Value, reply: Reply) -> Option<Immediate> {
        if !connected {
            return Some(Immediate(reply, Err("Le moteur ne répond pas".into())));
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = if args.is_object() { args } else { json!({}) };
        msg["id"] = json!(id);
        msg["cmd"] = json!(cmd);
        self.pending.insert(id, reply);
        out.send(msg.to_string());
        None
    }

    /// Réponse du moteur à la requête `id` (sa suite, à exécuter par l'appelant).
    pub fn answered(&mut self, id: u64) -> Option<Reply> {
        self.pending.remove(&id)
    }

    /// Connexion perdue : toutes les requêtes en cours échouent.
    pub fn fail_all(&mut self) -> Vec<Immediate> {
        self.pending.drain().map(|(_, r)| Immediate(r, Err("Connexion au moteur perdue".into()))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Faux système : garde ce qui serait envoyé au moteur.
    #[derive(Default)]
    struct Fake(RefCell<Vec<Value>>);

    impl Outbox for Fake {
        fn send(&self, msg: String) {
            self.0.borrow_mut().push(serde_json::from_str(&msg).unwrap());
        }
    }

    type Got = Rc<RefCell<Option<Result<Value, String>>>>;

    fn keep() -> (Got, Reply) {
        let got = Rc::new(RefCell::new(None));
        let g = got.clone();
        (got, Box::new(move |r| *g.borrow_mut() = Some(r)))
    }

    #[test]
    fn requests_go_out_and_get_their_answer() {
        let out = Fake::default();
        let mut c = Calls::new();
        let (got, reply) = keep();
        assert!(c.call(&out, true, "set_config_json", json!({ "config": { "a": 1 } }), reply).is_none());
        let m = out.0.borrow()[0].clone();
        assert_eq!(m, json!({ "id": 1, "cmd": "set_config_json", "config": { "a": 1 } }));
        c.answered(1).expect("requête en cours")(Ok(json!({ "rev": 3 })));
        assert_eq!(*got.borrow(), Some(Ok(json!({ "rev": 3 }))));
        assert!(c.answered(1).is_none());
    }

    #[test]
    fn offline_and_lost_connection_fail() {
        let out = Fake::default();
        let mut c = Calls::new();
        let (got, reply) = keep();
        c.call(&out, false, "get_config", json!({}), reply).unwrap().run();
        assert_eq!(*got.borrow(), Some(Err("Le moteur ne répond pas".into())));
        let (got, reply) = keep();
        c.call(&out, true, "discover", json!({}), reply);
        for i in c.fail_all() {
            i.run();
        }
        assert_eq!(*got.borrow(), Some(Err("Connexion au moteur perdue".into())));
    }
}
