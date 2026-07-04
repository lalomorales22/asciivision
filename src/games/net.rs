//! Network plumbing shared by the online-capable games (Pong, Tron).
//!
//! Games never touch sockets: they push `(game_name, payload)` pairs into an
//! abstract sender injected via `GamesPanel::set_net`, and receive inbound
//! payloads through `Game::handle_net`. The handshake here is a pure state
//! machine (returns payloads to send instead of sending them) so it is fully
//! unit-testable.
//!
//! Wire handshake (payloads ride inside the app's Game envelope):
//!   host  -> {"t":"invite"}                 (announce open room; re-sent every 2s)
//!   guest -> {"t":"join"}                   (blind join; re-sent every 2s while waiting)
//!   host  -> {"t":"start","seed":u64}       (locks the first joiner as opponent)
//!   host  -> {"t":"full"}                   (join rejected: room already locked)
//!   both  -> {"t":"ka"}                     (~1Hz liveness keepalive once locked)
//!   both  -> {"t":"quit"}                   (leaving; sent automatically on drop)
//! In-match traffic ({"t":"input",...} guest->host, {"t":"state",...} host->guest)
//! is game-specific and defined in pong.rs / tron.rs.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// Role a game instance plays in a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRole {
    Local,
    Host,
    Guest,
}

#[derive(Default)]
struct NetInner {
    tx: Option<UnboundedSender<(String, Value)>>,
    my_id: Option<String>,
}

/// Cheap clonable handle to the outbound game-message sender.
/// Shared between the panel and every live game session, so a late
/// `set_net` call reaches games that are already running.
#[derive(Clone, Default)]
pub(crate) struct NetHandle {
    inner: Arc<Mutex<NetInner>>,
}

impl NetHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn configure(
        &self,
        tx: Option<UnboundedSender<(String, Value)>>,
        my_id: Option<String>,
    ) {
        let mut inner = self.inner.lock();
        inner.tx = tx;
        inner.my_id = my_id;
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.inner.lock().tx.is_some()
    }

    pub(crate) fn my_id(&self) -> Option<String> {
        self.inner.lock().my_id.clone()
    }

    /// Send a payload for `game`; returns false when not connected.
    pub(crate) fn send(&self, game: &str, payload: Value) -> bool {
        let inner = self.inner.lock();
        match &inner.tx {
            Some(tx) => tx.send((game.to_string(), payload)).is_ok(),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Handshake state machine (pure: returns payloads instead of sending)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HsPhase {
    /// Host: waiting for a join. Guest: waiting for a start.
    Waiting,
    /// Opponent locked; match traffic may flow.
    Ready,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HsEvent {
    /// Both sides fire this once when the match is agreed. Seed is shared.
    Started { seed: u64 },
    /// The locked opponent sent {"t":"quit"}.
    OpponentLeft,
    /// A host replied {"t":"full"}: the room is locked to another guest.
    Full,
}

pub(crate) struct Handshake {
    pub(crate) role: NetRole,
    pub(crate) phase: HsPhase,
    pub(crate) seed: u64,
    pub(crate) opponent_id: Option<String>,
    pub(crate) opponent_name: String,
    resend: f32,
}

pub(crate) const HS_RESEND_SECS: f32 = 2.0;

impl Handshake {
    /// Start hosting: returns the handshake plus the invite payload to send.
    pub(crate) fn host(seed: u64) -> (Self, Value) {
        (
            Self {
                role: NetRole::Host,
                phase: HsPhase::Waiting,
                seed,
                opponent_id: None,
                opponent_name: String::new(),
                resend: 0.0,
            },
            json!({"t": "invite"}),
        )
    }

    /// Start joining: returns the handshake plus the join payload to send.
    pub(crate) fn guest() -> (Self, Value) {
        (
            Self {
                role: NetRole::Guest,
                phase: HsPhase::Waiting,
                seed: 0,
                opponent_id: None,
                opponent_name: String::new(),
                resend: 0.0,
            },
            json!({"t": "join"}),
        )
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.phase == HsPhase::Ready
    }

    /// True when `from_id` is the locked opponent.
    pub(crate) fn is_opponent(&self, from_id: &str) -> bool {
        self.opponent_id.as_deref() == Some(from_id)
    }

    /// Advance timers; may return a payload to (re)send while waiting.
    pub(crate) fn tick(&mut self, dt: f32) -> Option<Value> {
        if self.phase != HsPhase::Waiting {
            return None;
        }
        self.resend += dt;
        if self.resend < HS_RESEND_SECS {
            return None;
        }
        self.resend = 0.0;
        Some(match self.role {
            NetRole::Host => json!({"t": "invite"}),
            _ => json!({"t": "join"}),
        })
    }

    /// Feed an inbound payload. Returns (event, reply-to-send).
    pub(crate) fn on_net(
        &mut self,
        from_id: &str,
        from_name: &str,
        payload: &Value,
    ) -> (Option<HsEvent>, Option<Value>) {
        let tag = payload.get("t").and_then(Value::as_str).unwrap_or("");
        match (self.role, tag) {
            (NetRole::Host, "join") => {
                if self.phase == HsPhase::Waiting {
                    self.phase = HsPhase::Ready;
                    self.opponent_id = Some(from_id.to_string());
                    self.opponent_name = from_name.to_string();
                    (
                        Some(HsEvent::Started { seed: self.seed }),
                        Some(json!({"t": "start", "seed": self.seed})),
                    )
                } else if self.is_opponent(from_id) {
                    // Guest never saw our start (lost message): resend, no new event.
                    (None, Some(json!({"t": "start", "seed": self.seed})))
                } else {
                    // Room already locked to another guest: tell them so they
                    // stop waiting instead of resending join forever.
                    (None, Some(json!({"t": "full"})))
                }
            }
            (NetRole::Guest, "start") => {
                if self.phase == HsPhase::Waiting {
                    self.phase = HsPhase::Ready;
                    self.opponent_id = Some(from_id.to_string());
                    self.opponent_name = from_name.to_string();
                    self.seed = payload.get("seed").and_then(Value::as_u64).unwrap_or(0);
                    (Some(HsEvent::Started { seed: self.seed }), None)
                } else {
                    (None, None) // duplicate start: idempotent
                }
            }
            (NetRole::Guest, "invite") => {
                // A host appeared (possibly after we started waiting): answer fast.
                if self.phase == HsPhase::Waiting {
                    (None, Some(json!({"t": "join"})))
                } else {
                    (None, None)
                }
            }
            (NetRole::Guest, "full") => {
                // Our join was rejected: the host is locked to someone else.
                if self.phase == HsPhase::Waiting {
                    (Some(HsEvent::Full), None)
                } else {
                    (None, None)
                }
            }
            (_, "quit") => {
                if self.phase == HsPhase::Ready && self.is_opponent(from_id) {
                    (Some(HsEvent::OpponentLeft), None)
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        }
    }
}

// ---------------------------------------------------------------------------
// Send-rate throttle (accumulated dt)
// ---------------------------------------------------------------------------

pub(crate) struct Throttle {
    interval: f32,
    acc: f32,
}

impl Throttle {
    pub(crate) fn new(hz: f32) -> Self {
        Self {
            interval: 1.0 / hz.max(1.0),
            acc: 0.0,
        }
    }

    /// Accumulate `dt`; returns true when a send slot is due.
    pub(crate) fn ready(&mut self, dt: f32) -> bool {
        self.acc += dt;
        if self.acc >= self.interval {
            self.acc = 0.0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Peer liveness (keepalive pacing + inbound-silence timeout)
// ---------------------------------------------------------------------------

/// Seconds of inbound silence after which the locked opponent counts as gone.
pub(crate) const PEER_TIMEOUT_SECS: f32 = 5.0;
/// Rate at which each side proves it is still alive once a match is locked.
pub(crate) const KEEPALIVE_HZ: f32 = 1.0;

/// Tracks inbound traffic from the locked opponent and paces our own
/// keepalive sends. Pure timers, so the timeout state machine is testable.
pub(crate) struct Liveness {
    silence: f32,
    keepalive: Throttle,
}

impl Liveness {
    pub(crate) fn new() -> Self {
        Self {
            silence: 0.0,
            keepalive: Throttle::new(KEEPALIVE_HZ),
        }
    }

    /// Any payload from the opponent proves the peer is alive.
    pub(crate) fn on_inbound(&mut self) {
        self.silence = 0.0;
    }

    /// Advance timers. Returns `(keepalive_due, opponent_timed_out)`.
    pub(crate) fn tick(&mut self, dt: f32) -> (bool, bool) {
        self.silence += dt;
        (self.keepalive.ready(dt), self.silence > PEER_TIMEOUT_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_payload_shapes() {
        let (_hs, invite) = Handshake::host(42);
        assert_eq!(invite, json!({"t": "invite"}));
        let (_hs, join) = Handshake::guest();
        assert_eq!(join, json!({"t": "join"}));
    }

    #[test]
    fn invite_join_start_flow() {
        // Host opens a room.
        let (mut host, _invite) = Handshake::host(7);
        assert!(!host.is_ready());

        // Guest joins blind.
        let (mut guest, join) = Handshake::guest();

        // Host receives the join: locks opponent, emits Started + start payload.
        let (event, reply) = host.on_net("guest-id", "K7", &join);
        assert_eq!(event, Some(HsEvent::Started { seed: 7 }));
        let start = reply.expect("host must reply with start");
        assert_eq!(start, json!({"t": "start", "seed": 7}));
        assert!(host.is_ready());
        assert!(host.is_opponent("guest-id"));
        assert_eq!(host.opponent_name, "K7");

        // Guest receives the start: locks host, adopts the seed.
        let (event, reply) = guest.on_net("host-id", "HOSTY", &start);
        assert_eq!(event, Some(HsEvent::Started { seed: 7 }));
        assert!(reply.is_none());
        assert!(guest.is_ready());
        assert_eq!(guest.seed, 7);
        assert!(guest.is_opponent("host-id"));
    }

    #[test]
    fn host_rejects_second_joiner_with_full_but_resends_start_to_opponent() {
        let (mut host, _) = Handshake::host(1);
        let join = json!({"t": "join"});
        host.on_net("first", "A", &join);
        // A different client joining a locked room is told it is full.
        let (event, reply) = host.on_net("second", "B", &join);
        assert!(event.is_none());
        assert_eq!(reply, Some(json!({"t": "full"})));
        // The locked opponent re-joining (lost start) gets the start again.
        let (event, reply) = host.on_net("first", "A", &join);
        assert!(event.is_none());
        assert_eq!(reply, Some(json!({"t": "start", "seed": 1})));
    }

    #[test]
    fn waiting_guest_surfaces_full_rejection() {
        let (mut guest, _) = Handshake::guest();
        let full = json!({"t": "full"});
        let (event, reply) = guest.on_net("host-id", "H", &full);
        assert_eq!(event, Some(HsEvent::Full));
        assert!(reply.is_none());

        // Once locked to a host, a stray full is ignored.
        let (mut guest, _) = Handshake::guest();
        guest.on_net("host-id", "H", &json!({"t": "start", "seed": 4}));
        assert_eq!(guest.on_net("other", "O", &full).0, None);
    }

    #[test]
    fn quit_only_fires_for_locked_opponent() {
        let (mut host, _) = Handshake::host(3);
        let quit = json!({"t": "quit"});
        // Not ready yet: no event.
        assert_eq!(host.on_net("x", "X", &quit).0, None);
        host.on_net("opp", "OPP", &json!({"t": "join"}));
        // Stranger quitting: ignored.
        assert_eq!(host.on_net("x", "X", &quit).0, None);
        // Opponent quitting: event fires.
        assert_eq!(host.on_net("opp", "OPP", &quit).0, Some(HsEvent::OpponentLeft));
    }

    #[test]
    fn waiting_guest_answers_invite_and_resends_join() {
        let (mut guest, _) = Handshake::guest();
        let (event, reply) = guest.on_net("host-id", "H", &json!({"t": "invite"}));
        assert!(event.is_none());
        assert_eq!(reply, Some(json!({"t": "join"})));

        // Resend timer fires every HS_RESEND_SECS while waiting.
        assert!(guest.tick(HS_RESEND_SECS - 0.1).is_none());
        assert_eq!(guest.tick(0.2), Some(json!({"t": "join"})));

        // Once ready, no more resends.
        guest.on_net("host-id", "H", &json!({"t": "start", "seed": 9}));
        assert!(guest.tick(10.0).is_none());
    }

    #[test]
    fn duplicate_start_is_idempotent() {
        let (mut guest, _) = Handshake::guest();
        let start = json!({"t": "start", "seed": 5});
        assert!(guest.on_net("h", "H", &start).0.is_some());
        assert!(guest.on_net("h", "H", &start).0.is_none());
        assert_eq!(guest.seed, 5);
    }

    #[test]
    fn net_handle_send_and_connectivity() {
        let handle = NetHandle::new();
        assert!(!handle.is_connected());
        assert!(!handle.send("pong", json!({"t": "quit"})));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        handle.configure(Some(tx), Some("me".to_string()));
        assert!(handle.is_connected());
        assert_eq!(handle.my_id().as_deref(), Some("me"));
        assert!(handle.send("pong", json!({"t": "invite"})));
        let (game, payload) = rx.try_recv().expect("payload queued");
        assert_eq!(game, "pong");
        assert_eq!(payload, json!({"t": "invite"}));

        handle.configure(None, None);
        assert!(!handle.is_connected());
    }

    #[test]
    fn throttle_accumulates_dt() {
        let mut throttle = Throttle::new(20.0); // 50ms interval
        assert!(!throttle.ready(0.03));
        assert!(throttle.ready(0.03)); // 60ms accumulated
        assert!(!throttle.ready(0.04));
        assert!(throttle.ready(0.02)); // 60ms accumulated again
    }

    #[test]
    fn liveness_times_out_after_silence_and_resets_on_inbound() {
        let mut liveness = Liveness::new();
        // Just under the timeout: still alive.
        let mut timed_out = false;
        for _ in 0..49 {
            timed_out |= liveness.tick(0.1).1; // 4.9s total
        }
        assert!(!timed_out, "no timeout before {}s", PEER_TIMEOUT_SECS);
        // Inbound traffic resets the silence clock.
        liveness.on_inbound();
        for _ in 0..49 {
            timed_out |= liveness.tick(0.1).1;
        }
        assert!(!timed_out, "inbound traffic must reset the timeout");
        // Crossing the threshold with no inbound traffic times out.
        for _ in 0..3 {
            timed_out |= liveness.tick(0.1).1; // 5.2s since last inbound
        }
        assert!(timed_out, "silence past {}s times out", PEER_TIMEOUT_SECS);
    }

    #[test]
    fn liveness_paces_keepalives_at_one_hz() {
        let mut liveness = Liveness::new();
        let mut sends = 0;
        for _ in 0..30 {
            if liveness.tick(0.1).0 {
                sends += 1;
            }
        }
        assert_eq!(sends, 3, "3 seconds of ticks yield 3 keepalives");
    }
}
