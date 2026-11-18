//! Événements de l'hôte : émis de n'importe quel fil, livrés sur le fil de l'interface aux
//! abonnés de cet événement (tous, ceux d'une fenêtre, ou ceux d'un filtre d'étiquettes).
//!
//! La charge n'est pas sérialisée à l'envoi : l'abonné la lit telle quelle
//! (`event.payload::<T>()`), ou en JSON (`event.json()`) s'il ne connaît pas son type.

use serde::Serialize;
use serde_json::Value;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// Destinataires d'un événement.
pub(super) enum Target {
    All,
    Label(String),
    Filter(Box<dyn Fn(&str) -> bool + Send>),
}

impl Target {
    fn accepts(&self, label: Option<&str>) -> bool {
        match self {
            Target::All => true,
            Target::Label(l) => label == Some(l.as_str()),
            Target::Filter(f) => label.is_some_and(f),
        }
    }
}

/// Événement reçu par un abonné.
pub struct Event {
    name: String,
    payload: Arc<dyn Any + Send + Sync>,
    json: Arc<dyn Fn() -> Value + Send + Sync>,
}

impl Event {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Charge, si elle est de type `T`.
    pub fn payload<T: 'static>(&self) -> Option<&T> {
        self.payload.downcast_ref()
    }

    /// Charge en JSON (comme la recevait l'interface web).
    pub fn json(&self) -> Value {
        (self.json)()
    }
}

/// Abonnement (pour `unlisten`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// Abonné (fil de l'interface).
type Handler = Rc<dyn Fn(&Event)>;

struct Listener {
    id: ListenerId,
    event: String,
    /// Fenêtre de l'abonné (`None` : l'application, qui ne reçoit que les envois à tous).
    label: Option<String>,
    /// Liaison qui l'a posé (`scoped`) : retiré avec elle (`unlisten_scope`).
    scope: Option<String>,
    handler: Handler,
}

thread_local! {
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(0) };
    /// Liaison en cours (`scoped`).
    static SCOPE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Abonne `handler` à `event` (fil de l'interface). `label` : fenêtre de l'abonné, qui reçoit
/// aussi ce qui est envoyé à elle seule ; ses abonnements partent avec elle (`unlisten_window`).
pub fn listen(event: &str, label: Option<&str>, handler: impl Fn(&Event) + 'static) -> ListenerId {
    let id = ListenerId(NEXT.replace(NEXT.get() + 1));
    LISTENERS.with_borrow_mut(|l| {
        l.push(Listener {
            id,
            event: event.to_owned(),
            label: label.map(str::to_owned),
            scope: SCOPE.with_borrow(Clone::clone),
            handler: Rc::new(handler),
        })
    });
    id
}

pub fn unlisten(id: ListenerId) {
    LISTENERS.with_borrow_mut(|l| l.retain(|x| x.id != id));
}

/// Retire les abonnements de la fenêtre `label` (fermée).
pub fn unlisten_window(label: &str) {
    let _ = LISTENERS.try_with(|l| l.borrow_mut().retain(|x| x.label.as_deref() != Some(label)));
}

/// Appelle `f` en notant `scope` sur les abonnements qu'il pose : ils partent ensemble avec
/// `unlisten_scope` (liaison d'un widget refaite quand la disposition de sa barre change).
pub fn scoped<R>(scope: &str, f: impl FnOnce() -> R) -> R {
    let previous = SCOPE.replace(Some(scope.to_owned()));
    let out = f();
    SCOPE.set(previous);
    out
}

/// Retire les abonnements posés sous `scope`.
pub fn unlisten_scope(scope: &str) {
    let _ = LISTENERS.try_with(|l| l.borrow_mut().retain(|x| x.scope.as_deref() != Some(scope)));
}

pub(super) fn send<S: Serialize + Send + Sync + 'static>(name: &str, payload: S, target: Target) -> anyhow::Result<()> {
    let payload = Arc::new(payload);
    let for_json = payload.clone();
    let event = Event {
        name: name.to_owned(),
        payload,
        json: Arc::new(move || serde_json::to_value(&*for_json).unwrap_or(Value::Null)),
    };
    slint::invoke_from_event_loop(move || deliver(&event, &target)).map_err(|e| anyhow::anyhow!("événement {name} : {e}"))
}

/// Appelle les abonnés concernés (copiés d'abord : un abonné peut s'abonner ou se désabonner).
fn deliver(event: &Event, target: &Target) {
    let handlers: Vec<Handler> = LISTENERS.with_borrow(|l| {
        l.iter()
            .filter(|x| x.event == event.name && target.accepts(x.label.as_deref()))
            .map(|x| x.handler.clone())
            .collect()
    });
    for h in handlers {
        h(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, v: u32) -> Event {
        let payload = Arc::new(v);
        let p = payload.clone();
        Event { name: name.into(), payload, json: Arc::new(move || serde_json::json!(*p)) }
    }

    #[test]
    fn delivery_follows_the_target() {
        let got: Rc<RefCell<Vec<String>>> = Rc::default();
        for label in [None, Some("bar-1-0"), Some("bar-1-1")] {
            let g = got.clone();
            let name = label.unwrap_or("app").to_owned();
            listen("horizon://x", label, move |e| {
                assert_eq!(e.payload::<u32>(), Some(&7));
                assert_eq!(e.json(), serde_json::json!(7));
                g.borrow_mut().push(name.clone());
            });
        }
        let id = listen("horizon://autre", None, |_| panic!("autre événement"));
        deliver(&event("horizon://x", 7), &Target::All);
        assert_eq!(got.take(), ["app", "bar-1-0", "bar-1-1"]);
        deliver(&event("horizon://x", 7), &Target::Label("bar-1-1".into()));
        assert_eq!(got.take(), ["bar-1-1"]);
        deliver(&event("horizon://x", 7), &Target::Filter(Box::new(|l: &str| l.ends_with("-0"))));
        assert_eq!(got.take(), ["bar-1-0"]);
        unlisten_window("bar-1-0");
        unlisten(id);
        deliver(&event("horizon://x", 7), &Target::All);
        assert_eq!(got.take(), ["app", "bar-1-1"]);
    }

    #[test]
    fn scoped_listeners_leave_together() {
        let got: Rc<RefCell<Vec<&str>>> = Rc::default();
        let g = got.clone();
        scoped("bar-2-0#weather", || {
            let g = g.clone();
            listen("horizon://s", Some("bar-2-0"), move |_| g.borrow_mut().push("lié"));
        });
        listen("horizon://s", Some("bar-2-0"), move |_| g.borrow_mut().push("libre"));
        deliver(&event("horizon://s", 1), &Target::All);
        assert_eq!(got.take(), ["lié", "libre"]);
        unlisten_scope("bar-2-0#weather");
        deliver(&event("horizon://s", 1), &Target::All);
        assert_eq!(got.take(), ["libre"]);
        unlisten_window("bar-2-0");
    }
}
