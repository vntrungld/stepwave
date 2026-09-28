//! Per-app routing decisions, independent of PipeWire. The PipeWire glue (`graph.rs`)
//! feeds events in and applies the returned actions.
//!
//! A stream's process binary lives on its *client* (`application.process.binary`), not
//! on the node, and the client's info can arrive after the stream node appears; both
//! orders are handled.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A client's info arrived (or changed).
    Client {
        id: u32,
        binary: String,
    },
    ClientRemoved {
        id: u32,
    },
    /// An output audio stream node appeared.
    Stream {
        id: u32,
        client: u32,
    },
    StreamRemoved {
        id: u32,
    },
    /// Someone set (or cleared) `target.object` for a stream in the default metadata.
    Target {
        stream: u32,
        target: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Set the stream's `target.object` to the stepwave sink.
    Route { stream: u32 },
    /// A newly routed stream selects its profile (most recent wins).
    SelectProfile { id: String },
}

#[derive(Debug, Clone)]
struct StreamInfo {
    client: u32,
    /// We have routed it.
    routed: bool,
    /// The user moved it elsewhere after we routed it: leave it alone until it ends.
    manual: bool,
}

pub struct Router {
    sink_name: String,
    /// binary -> profile id
    matches: HashMap<String, String>,
    clients: HashMap<u32, String>,
    streams: HashMap<u32, StreamInfo>,
}

impl Router {
    pub fn new(sink_name: &str, matches: HashMap<String, String>) -> Self {
        Router {
            sink_name: sink_name.to_string(),
            matches,
            clients: HashMap::new(),
            streams: HashMap::new(),
        }
    }

    /// Replace the binary -> profile table (after `reload`). Already-routed streams stay.
    pub fn set_matches(&mut self, matches: HashMap<String, String>) {
        self.matches = matches;
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Client { id, binary } => {
                self.clients.insert(id, binary);
                let waiting: Vec<u32> = self
                    .streams
                    .iter()
                    .filter(|(_, s)| s.client == id && !s.routed && !s.manual)
                    .map(|(&sid, _)| sid)
                    .collect();
                waiting
                    .into_iter()
                    .flat_map(|sid| self.try_route(sid))
                    .collect()
            }
            Event::ClientRemoved { id } => {
                self.clients.remove(&id);
                Vec::new()
            }
            Event::Stream { id, client } => {
                self.streams.insert(
                    id,
                    StreamInfo {
                        client,
                        routed: false,
                        manual: false,
                    },
                );
                self.try_route(id)
            }
            Event::StreamRemoved { id } => {
                self.streams.remove(&id);
                Vec::new()
            }
            Event::Target { stream, target } => {
                if let Some(s) = self.streams.get_mut(&stream) {
                    if s.routed && target.as_deref() != Some(self.sink_name.as_str()) {
                        s.manual = true;
                    }
                }
                Vec::new()
            }
        }
    }

    fn try_route(&mut self, stream: u32) -> Vec<Action> {
        let Some(info) = self.streams.get(&stream) else {
            return Vec::new();
        };
        if info.routed || info.manual {
            return Vec::new();
        }
        let Some(binary) = self.clients.get(&info.client) else {
            return Vec::new();
        };
        let Some(profile) = self.matches.get(binary).cloned() else {
            return Vec::new();
        };
        if let Some(s) = self.streams.get_mut(&stream) {
            s.routed = true;
        }
        vec![
            Action::Route { stream },
            Action::SelectProfile { id: profile },
        ]
    }

    /// Streams currently routed into the sink (not manually moved away), sorted by id.
    pub fn routed(&self) -> Vec<(u32, String)> {
        let mut v: Vec<(u32, String)> = self
            .streams
            .iter()
            .filter(|(_, s)| s.routed && !s.manual)
            .map(|(&id, s)| (id, self.clients.get(&s.client).cloned().unwrap_or_default()))
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> Router {
        let matches = [("cs2", "cs2"), ("pubg", "pubg")]
            .into_iter()
            .map(|(b, p)| (b.to_string(), p.to_string()))
            .collect();
        Router::new("stepwave", matches)
    }

    fn client(id: u32, binary: &str) -> Event {
        Event::Client {
            id,
            binary: binary.into(),
        }
    }

    #[test]
    fn routes_matching_stream_when_client_known_first() {
        let mut r = router();
        assert!(r.handle(client(10, "cs2")).is_empty());
        assert_eq!(
            r.handle(Event::Stream { id: 50, client: 10 }),
            vec![
                Action::Route { stream: 50 },
                Action::SelectProfile { id: "cs2".into() }
            ]
        );
        assert_eq!(r.routed(), vec![(50, "cs2".to_string())]);
    }

    #[test]
    fn routes_when_client_info_arrives_late() {
        let mut r = router();
        assert!(r.handle(Event::Stream { id: 50, client: 10 }).is_empty());
        assert_eq!(
            r.handle(client(10, "cs2")),
            vec![
                Action::Route { stream: 50 },
                Action::SelectProfile { id: "cs2".into() }
            ]
        );
    }

    #[test]
    fn ignores_non_matching_streams_and_routes_once() {
        let mut r = router();
        r.handle(client(10, "firefox"));
        assert!(r.handle(Event::Stream { id: 50, client: 10 }).is_empty());
        r.handle(client(11, "cs2"));
        assert_eq!(r.handle(Event::Stream { id: 51, client: 11 }).len(), 2);
        // client info repeated: no second route
        assert!(r.handle(client(11, "cs2")).is_empty());
        assert_eq!(r.routed(), vec![(51, "cs2".to_string())]);
    }

    #[test]
    fn most_recent_matching_stream_selects_profile() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(client(11, "pubg"));
        r.handle(Event::Stream { id: 50, client: 10 });
        let actions = r.handle(Event::Stream { id: 51, client: 11 });
        assert_eq!(actions[1], Action::SelectProfile { id: "pubg".into() });
    }

    #[test]
    fn manual_move_is_respected_until_stream_ends() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(Event::Stream { id: 50, client: 10 });
        // our own write echoes back: not manual
        r.handle(Event::Target {
            stream: 50,
            target: Some("stepwave".into()),
        });
        assert_eq!(r.routed().len(), 1);
        // user moves it to the speakers
        r.handle(Event::Target {
            stream: 50,
            target: Some("alsa_output.x".into()),
        });
        assert!(r.routed().is_empty());
        assert!(r.handle(client(10, "cs2")).is_empty());
        // stream ends; a new stream from the same game is routed again
        r.handle(Event::StreamRemoved { id: 50 });
        assert_eq!(r.handle(Event::Stream { id: 52, client: 10 }).len(), 2);
    }

    #[test]
    fn reload_changes_future_matches_only() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(Event::Stream { id: 50, client: 10 });
        r.set_matches(HashMap::new());
        assert_eq!(r.routed().len(), 1);
        r.handle(client(11, "cs2"));
        assert!(r.handle(Event::Stream { id: 51, client: 11 }).is_empty());
    }
}
