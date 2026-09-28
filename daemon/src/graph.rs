//! PipeWire glue for routing: turns registry and metadata events into `router::Event`s and
//! applies the resulting actions. Runs on the main loop thread only.
//!
//! Facts this relies on (checked against PipeWire 1.6 / WirePlumber 0.5):
//! - `application.process.binary` is on the *client* info, not on the stream node or its
//!   registry global; the node's global carries `client.id`.
//! - Setting `target.object` = the sink's node name in the `default` metadata makes
//!   WirePlumber move the stream; when the sink disappears the stream returns to the
//!   default device, and nothing is persisted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pipewire as pw;
use pw::client::{Client, ClientListener};
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::{Listener as RegistryListener, RegistryRc};
use pw::types::ObjectType;

use crate::node::SINK_NAME;
use crate::router::{Action, Event, Router};

type OnSelect = Box<dyn Fn(&str)>;

struct State {
    router: Router,
    /// Listener before the object it listens on, so it unhooks first on drop
    /// (see the comment on `Graph` below).
    metadata: Option<(MetadataListener, Metadata)>,
    clients: HashMap<u32, (ClientListener, Client)>,
    /// Routes decided before the `default` metadata was bound.
    pending: Vec<u32>,
    on_select: OnSelect,
}

/// Field order is drop order: the registry listener must unhook before the
/// registry itself is destroyed, for the same reason as `node::Node` (a
/// listener dropped after the object it watches is a use-after-free).
pub struct Graph {
    _listener: RegistryListener,
    _registry: RegistryRc,
    state: Rc<RefCell<State>>,
}

impl Graph {
    /// Streams currently routed into the sink: `(node id, process binary)`.
    pub fn routed(&self) -> Vec<(u32, String)> {
        self.state.borrow().router.routed()
    }

    /// New binary -> profile table (after a reload).
    pub fn set_matches(&self, matches: HashMap<String, String>) {
        self.state.borrow_mut().router.set_matches(matches);
    }
}

/// Feed one event through the router and apply its actions.
fn dispatch(state: &Rc<RefCell<State>>, event: Event) {
    let actions = state.borrow_mut().router.handle(event);
    for action in actions {
        match action {
            Action::Route { stream } => {
                let st = state.borrow();
                if let Some((_, md)) = st.metadata.as_ref() {
                    md.set_property(stream, "target.object", None, Some(SINK_NAME));
                } else {
                    drop(st);
                    state.borrow_mut().pending.push(stream);
                }
            }
            Action::SelectProfile { id } => (state.borrow().on_select)(&id),
        }
    }
}

pub fn create(
    core: &pw::core::CoreRc,
    router: Router,
    on_select: impl Fn(&str) + 'static,
) -> Result<Graph, pw::Error> {
    let registry = core.get_registry_rc()?;
    let state = Rc::new(RefCell::new(State {
        router,
        metadata: None,
        clients: HashMap::new(),
        pending: Vec::new(),
        on_select: Box::new(on_select),
    }));

    let reg_weak = registry.downgrade();
    let st_global = state.clone();
    let st_remove = state.clone();
    let listener = registry
        .add_listener_local()
        .global(move |g| {
            let Some(props) = g.props else { return };
            let Some(reg) = reg_weak.upgrade() else {
                return;
            };
            match g.type_ {
                ObjectType::Client => {
                    let Ok(client) = reg.bind::<Client, _>(g) else {
                        return;
                    };
                    let id = g.id;
                    // Weak: `State` owns this listener, so a strong `Rc` here would
                    // be a cycle and leak `State` on every reconnect.
                    let st = Rc::downgrade(&st_global);
                    let l = client
                        .add_listener_local()
                        .info(move |info| {
                            let Some(st) = st.upgrade() else { return };
                            let binary = info
                                .props()
                                .and_then(|p| p.get("application.process.binary"))
                                .map(str::to_string);
                            if let Some(binary) = binary {
                                dispatch(&st, Event::Client { id, binary });
                            }
                        })
                        .register();
                    st_global.borrow_mut().clients.insert(id, (l, client));
                }
                ObjectType::Node if props.get("media.class") == Some("Stream/Output/Audio") => {
                    if let Some(client) = props.get("client.id").and_then(|c| c.parse().ok()) {
                        dispatch(&st_global, Event::Stream { id: g.id, client });
                    }
                }
                ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
                    let Ok(md) = reg.bind::<Metadata, _>(g) else {
                        return;
                    };
                    // Weak for the same reason as the client listener above.
                    let st = Rc::downgrade(&st_global);
                    let l = md
                        .add_listener_local()
                        .property(move |subject, key, _type, value| {
                            let Some(st) = st.upgrade() else { return 0 };
                            if key == Some("target.object") {
                                let target = value.map(str::to_string);
                                dispatch(
                                    &st,
                                    Event::Target {
                                        stream: subject,
                                        target,
                                    },
                                );
                            }
                            0
                        })
                        .register();
                    let pending = std::mem::take(&mut st_global.borrow_mut().pending);
                    for stream in pending {
                        md.set_property(stream, "target.object", None, Some(SINK_NAME));
                    }
                    st_global.borrow_mut().metadata = Some((l, md));
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            st_remove.borrow_mut().clients.remove(&id);
            dispatch(&st_remove, Event::StreamRemoved { id });
            dispatch(&st_remove, Event::ClientRemoved { id });
        })
        .register();

    Ok(Graph {
        _listener: listener,
        _registry: registry,
        state,
    })
}
