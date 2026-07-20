//! Playback-session lifecycle and the audio control loop.
//!
//! This module owns *when* a playback session starts, is superseded, and is torn
//! down. It deliberately knows nothing about CPAL, Symphonia, or HTTP: the
//! device- and network-specific wiring is supplied by a [`PlaybackEngine`]
//! implementation (the production one lives in `src/audio.rs`). That split is
//! what makes the concurrency behavior testable with a fake blocking engine,
//! without a real audio device or network.
//!
//! The responsiveness rule this module exists to enforce: **the control thread
//! must never block on a network or decoder read.** Two things used to violate
//! it — connecting, and joining a torn-down session's workers — so both are
//! moved off the control thread. See [`run_control_loop`].

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::model::{PlaybackRequestId, Station, StationId, VolumePercent};

use super::output::SharedVolume;
use super::{AudioCommand, AudioEvent};

/// How often the control thread checks for a finished connect while one is in
/// flight. Only used while connects are outstanding; otherwise the thread parks
/// on the command channel. This bounds how late a *successful* connect is
/// noticed, not how fast a command is received.
const CONNECT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Ceiling on concurrently running connect workers.
///
/// A connect worker cannot be cancelled, so each one that is started against a
/// wedged station occupies a thread (and a socket) until its read timeout
/// expires. Without a ceiling, holding down Enter would start one per keypress.
///
/// The ceiling is not a queue: only the *newest* request is ever waiting for a
/// slot, because a superseded request has no one left to play it. Rapid input
/// therefore costs at most this many lingering workers plus one for the request
/// the user actually settled on. The value is small but greater than one so the
/// common case — changing your mind once or twice while a station is slow —
/// still connects immediately rather than waiting for a timeout.
const MAX_CONNECT_WORKERS: usize = 4;

/// The device/network-specific half of playback, split so the blocking part can
/// run off the control thread.
///
/// The two phases exist for one reason: [`connect`] may block for as long as the
/// configured network timeouts allow, so it runs on a throwaway worker thread,
/// while [`start`] builds the output stream and must run on the control thread
/// because the stream is not `Send` and its lifetime is control-thread-owned.
///
/// [`connect`]: PlaybackEngine::connect
/// [`start`]: PlaybackEngine::start
pub(super) trait PlaybackEngine: Send + Sync + 'static {
    /// The connected-but-not-yet-playing stream source. Crosses from the connect
    /// worker back to the control thread, so it must be `Send`.
    type Connected: Send + 'static;
    /// The live output stream. Never crosses threads; dropping it stops output.
    type Stream;

    /// Open the station's stream. Runs on a connect worker thread and may block
    /// for the configured connect/read timeouts.
    fn connect(
        &self,
        request: PlaybackRequestId,
        station: Station,
        events: Sender<AudioEvent>,
    ) -> anyhow::Result<Self::Connected>;

    /// Start output and analysis for an already-connected stream. Runs on the
    /// control thread and must not perform network reads.
    fn start(
        &self,
        request: PlaybackRequestId,
        station: StationId,
        connected: Self::Connected,
        volume: &SharedVolume,
        events: &Sender<AudioEvent>,
    ) -> anyhow::Result<(Self::Stream, SessionWorkers)>;
}

/// The `Send` half of a playback session: its stop flag and worker threads.
///
/// Kept separate from the output stream precisely so it can be handed to the
/// [`Reaper`] while the stream stays on the control thread.
pub(super) struct SessionWorkers {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl SessionWorkers {
    /// Workers that observe `stop` to exit.
    pub(super) fn new(stop: Arc<AtomicBool>, threads: Vec<JoinHandle<()>>) -> Self {
        Self { stop, threads }
    }

    /// A single thread with no stop flag of its own — a connect worker, which
    /// cannot be interrupted and simply has to be joined once it returns.
    fn unstoppable(thread: JoinHandle<()>) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            threads: vec![thread],
        }
    }

    /// Ask the workers to stop. Non-blocking; workers already inside an
    /// uninterruptible read observe this only once that read returns.
    fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Background joiner for torn-down sessions.
///
/// Retiring is what the control thread does instead of joining: the stop flag is
/// raised immediately and the threads are handed here, so command receipt never
/// waits on a worker that is parked in a blocking read.
///
/// The reaper joins retirements in the order it receives them. A retirement
/// queued behind a worker that is still inside a bounded blocking read is
/// reclaimed later, not never: it has already been cancelled and exits on its
/// own, so nothing leaks — only the `join` is deferred.
pub(super) struct Reaper {
    /// `None` only after [`Reaper::join`] has taken it.
    tx: Option<Sender<SessionWorkers>>,
    thread: Option<JoinHandle<()>>,
}

impl Reaper {
    pub(super) fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<SessionWorkers>();
        let thread = thread::spawn(move || {
            while let Ok(mut workers) = rx.recv() {
                workers.cancel();
                for thread in workers.threads.drain(..) {
                    let _ = thread.join();
                }
            }
        });
        Self {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    /// Cancel `workers` and hand them off to be joined in the background.
    /// Never blocks.
    fn retire(&self, workers: SessionWorkers) {
        workers.cancel();
        if let Some(tx) = self.tx.as_ref() {
            // A send failure means the reaper thread is gone; the workers are
            // already cancelled and will exit on their own.
            let _ = tx.send(workers);
        }
    }

    /// Close the queue and wait for every outstanding retirement to be joined.
    ///
    /// Only used by tests to assert leak-freedom deterministically. Production
    /// shutdown deliberately does *not* call this: `Shutdown` must return
    /// without waiting for a blocked read, so the reaper is left to drain and
    /// exit on its own (see [`Reaper::drop`]).
    #[cfg(test)]
    fn join(mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Reaper {
    /// Closes the queue and *detaches* the reaper thread rather than joining it,
    /// so control-loop shutdown stays prompt. The reaper finishes its backlog
    /// and exits on its own; all it holds are already-cancelled workers.
    fn drop(&mut self) {
        drop(self.tx.take());
        drop(self.thread.take());
    }
}

/// A started session: the control-thread-owned stream plus its workers.
struct LiveSession<S> {
    stream: S,
    workers: SessionWorkers,
}

/// Tear `live` down without blocking the control thread.
///
/// Order matters: cancel first so workers see the flag as soon as their current
/// read returns, then drop the stream (this is the part that actually silences
/// output, and it is a bounded device call, never a network read), then hand the
/// threads to the reaper.
fn retire<S>(live: LiveSession<S>, reaper: &Reaper) {
    let LiveSession { stream, workers } = live;
    workers.cancel();
    drop(stream);
    reaper.retire(workers);
}

/// The play request the runtime currently intends to satisfy.
///
/// There is at most one: a newer `Play` overwrites it, which is what coalesces
/// rapid input down to the request the user settled on. `spawned` records
/// whether a connect worker is actually running for it — under
/// [`MAX_CONNECT_WORKERS`] pressure a request is accepted and announced before
/// it can be connected, and is started as soon as a slot frees.
struct Pending {
    request: PlaybackRequestId,
    station: Station,
    spawned: bool,
}

impl Pending {
    fn station_id(&self) -> StationId {
        self.station.id.clone()
    }
}

/// The result of one connect worker, tagged with the request that asked for it.
struct ConnectOutcome<C> {
    request: PlaybackRequestId,
    result: anyhow::Result<C>,
}

/// The control thread body: own the live session and volume, and turn commands
/// into playback actions and events.
///
/// Responsiveness invariants:
///
/// - Connecting runs on a worker thread; the control thread only polls for its
///   outcome, so a wedged station cannot delay command receipt.
/// - Teardown hands workers to the [`Reaper`]; the control thread joins nothing.
/// - A connect outcome is adopted only if its [`PlaybackRequestId`] still
///   matches the pending request. Anything else is dropped — which also closes
///   the stream it carries — so a completion that races a `Stop`, a replacement
///   `Play`, or a `Shutdown` can never restore stale playback state.
pub(super) fn run_control_loop<E: PlaybackEngine>(
    engine: E,
    command_rx: Receiver<AudioCommand>,
    event_tx: Sender<AudioEvent>,
) {
    let engine = Arc::new(engine);
    let reaper = Reaper::spawn();
    // Volume persists across stations; `Play` overrides it with its own value.
    let volume = SharedVolume::new(VolumePercent::clamped(100));
    let (connect_tx, connect_rx) = mpsc::channel::<ConnectOutcome<E::Connected>>();

    let mut current: Option<LiveSession<E::Stream>> = None;
    let mut pending: Option<Pending> = None;
    // Connects whose outcome has not been received yet, superseded ones
    // included. While any are outstanding the control thread polls instead of
    // parking, so a stale outcome is drained (and its stream closed) promptly.
    let mut outstanding_connects = 0usize;

    loop {
        while let Ok(outcome) = connect_rx.try_recv() {
            outstanding_connects = outstanding_connects.saturating_sub(1);
            // The request-identity gate: anything that is no longer the pending
            // request is superseded or cancelled, and dropping the outcome here
            // closes the stream it carries. No event is emitted for an attempt
            // the app has already left.
            let Some(attempt) = pending.take_if(|p| p.request == outcome.request) else {
                // A slot just freed; a request that was waiting for one starts now.
                try_spawn_pending(
                    &mut pending,
                    &mut outstanding_connects,
                    &engine,
                    &reaper,
                    &event_tx,
                    &connect_tx,
                );
                continue;
            };
            let request = attempt.request;
            let station = attempt.station_id();
            let started = outcome.result.and_then(|connected| {
                engine.start(request, station.clone(), connected, &volume, &event_tx)
            });
            match started {
                Ok((stream, workers)) => {
                    current = Some(LiveSession { stream, workers });
                    let _ = event_tx.send(AudioEvent::Playing { request, station });
                }
                Err(err) => {
                    let _ = event_tx.send(AudioEvent::Failed {
                        request,
                        station,
                        message: format!("{err:#}"),
                    });
                }
            }
        }

        let command = if outstanding_connects > 0 {
            match command_rx.recv_timeout(CONNECT_POLL_INTERVAL) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match command_rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };

        match command {
            AudioCommand::Play {
                request,
                station,
                volume: v,
            } => {
                // Stop the previous stream before announcing the new one. The old
                // session's workers may still emit events after this point; they
                // carry the old request and the app drops them.
                if let Some(live) = current.take() {
                    retire(live, &reaper);
                }
                let _ = event_tx.send(AudioEvent::Connecting {
                    request,
                    station: station.id.clone(),
                });
                volume.set(v);
                // Overwriting `pending` supersedes any in-flight connect: its
                // outcome no longer matches and is discarded when it arrives.
                // The request is announced as Connecting either way; whether a
                // worker starts now or when a slot frees is invisible to the app.
                pending = Some(Pending {
                    request,
                    station: *station,
                    spawned: false,
                });
                try_spawn_pending(
                    &mut pending,
                    &mut outstanding_connects,
                    &engine,
                    &reaper,
                    &event_tx,
                    &connect_tx,
                );
            }
            AudioCommand::Stop => {
                if let Some(live) = current.take() {
                    retire(live, &reaper);
                }
                pending = None;
                let _ = event_tx.send(AudioEvent::Stopped);
            }
            AudioCommand::SetVolume(v) => {
                volume.set(v);
                let _ = event_tx.send(AudioEvent::VolumeChanged(v));
            }
            AudioCommand::Shutdown => break,
        }
    }

    // Any still-live session is torn down here; dropping `connect_rx` makes an
    // outstanding connect worker discard its result (closing the stream it
    // opened) when it finally returns. Dropping `reaper` closes its queue
    // without waiting, so shutdown is not delayed by a blocked read.
    if let Some(live) = current.take() {
        retire(live, &reaper);
    }
}

/// Start a connect worker for the pending request, if one is warranted and a
/// slot is free.
///
/// Called both when a request arrives and when a worker finishes, so a request
/// held back by [`MAX_CONNECT_WORKERS`] starts as soon as a slot frees rather
/// than waiting for further input.
fn try_spawn_pending<E: PlaybackEngine>(
    pending: &mut Option<Pending>,
    outstanding_connects: &mut usize,
    engine: &Arc<E>,
    reaper: &Reaper,
    event_tx: &Sender<AudioEvent>,
    connect_tx: &Sender<ConnectOutcome<E::Connected>>,
) {
    let Some(attempt) = pending.as_mut() else {
        return;
    };
    if attempt.spawned || *outstanding_connects >= MAX_CONNECT_WORKERS {
        return;
    }
    attempt.spawned = true;
    *outstanding_connects += 1;
    spawn_connect(
        Arc::clone(engine),
        reaper,
        attempt.request,
        attempt.station.clone(),
        event_tx.clone(),
        connect_tx.clone(),
    );
}

/// Run one connect on its own thread and report the outcome back.
///
/// The worker is retired to the reaper immediately: it cannot be interrupted, so
/// the only guarantee available is that it is bounded by the engine's configured
/// timeouts and is joined once it returns.
///
/// A panic in the engine is caught and reported as a failed outcome. The control
/// loop accounts for every started worker by its outcome, so a worker that died
/// without reporting would strand the request as permanently pending and leak a
/// connect slot; turning the panic into a recoverable failure keeps both the
/// bookkeeping and the app's playback state honest.
fn spawn_connect<E: PlaybackEngine>(
    engine: Arc<E>,
    reaper: &Reaper,
    request: PlaybackRequestId,
    station: Station,
    events: Sender<AudioEvent>,
    outcomes: Sender<ConnectOutcome<E::Connected>>,
) {
    let thread = thread::spawn(move || {
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.connect(request, station, events)
        }));
        let result = attempt.unwrap_or_else(|_| Err(anyhow::anyhow!("connect worker panicked")));
        // A closed channel means the control loop moved on or shut down; the
        // result is dropped here instead, which closes the stream.
        let _ = outcomes.send(ConnectOutcome { request, result });
    });
    reaper.retire(SessionWorkers::unstoppable(thread));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BitrateKbps, CodecKind, PlaybackRequestSeq, StationName, StationSource, StreamUrl,
    };
    use std::marker::PhantomData;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    /// Upper bound for "the control thread answered without waiting on I/O".
    ///
    /// The fake's blocking points never time out on their own, so a control
    /// thread that waited on one would still be waiting when this elapses. The
    /// bound is generous: it only has to be shorter than "forever".
    const RESPONSIVE: Duration = Duration::from_secs(5);
    /// Bound for background reclamation once a blocked worker is released.
    const RECLAIM: Duration = Duration::from_secs(5);

    /// A manual-reset latch. Once opened it stays open.
    struct Gate {
        open: Mutex<bool>,
        cv: Condvar,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                open: Mutex::new(false),
                cv: Condvar::new(),
            }
        }

        fn opened() -> Self {
            Self {
                open: Mutex::new(true),
                cv: Condvar::new(),
            }
        }

        fn open(&self) {
            let mut open = self.open.lock().unwrap();
            *open = true;
            self.cv.notify_all();
        }

        /// Block until opened. Models an uninterruptible blocking read.
        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.cv.wait(open).unwrap();
            }
        }
    }

    /// Poll `pred` until it holds or `timeout` elapses.
    fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if pred() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// How the fake engine's `connect` ends once its gate opens.
    #[derive(Clone, Copy, PartialEq)]
    enum ConnectBehavior {
        Succeed,
        Fail,
        Panic,
    }

    /// Shared observation/control surface for the fake engine.
    struct FakeState {
        behavior: Mutex<ConnectBehavior>,
        start_fails: AtomicBool,
        connects_entered: AtomicUsize,
        /// Held closed to model a station that accepts the connection and then
        /// never sends anything.
        connect_gate: Gate,
        /// Held closed to model a session worker parked in a blocking read that
        /// does not observe the stop flag until the read returns.
        worker_gate: Gate,
        connects_finished: AtomicUsize,
        started: Mutex<Vec<PlaybackRequestId>>,
        live_workers: AtomicUsize,
        live_streams: AtomicUsize,
    }

    impl FakeState {
        fn new(connect_gate: Gate, worker_gate: Gate) -> Arc<Self> {
            Arc::new(Self {
                behavior: Mutex::new(ConnectBehavior::Succeed),
                start_fails: AtomicBool::new(false),
                connects_entered: AtomicUsize::new(0),
                connect_gate,
                worker_gate,
                connects_finished: AtomicUsize::new(0),
                started: Mutex::new(Vec::new()),
                live_workers: AtomicUsize::new(0),
                live_streams: AtomicUsize::new(0),
            })
        }

        fn started(&self) -> Vec<PlaybackRequestId> {
            self.started.lock().unwrap().clone()
        }

        fn live_workers(&self) -> usize {
            self.live_workers.load(Ordering::SeqCst)
        }

        fn live_streams(&self) -> usize {
            self.live_streams.load(Ordering::SeqCst)
        }

        fn connects_finished(&self) -> usize {
            self.connects_finished.load(Ordering::SeqCst)
        }

        fn connects_entered(&self) -> usize {
            self.connects_entered.load(Ordering::SeqCst)
        }

        fn set_behavior(&self, behavior: ConnectBehavior) {
            *self.behavior.lock().unwrap() = behavior;
        }
    }

    /// Stands in for the CPAL stream: not `Send`, so this fake would fail to
    /// compile if the design ever moved the live stream off the control thread.
    struct FakeStream {
        state: Arc<FakeState>,
        _not_send: PhantomData<*const ()>,
    }

    impl Drop for FakeStream {
        fn drop(&mut self) {
            self.state.live_streams.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct FakeEngine {
        state: Arc<FakeState>,
    }

    impl PlaybackEngine for FakeEngine {
        type Connected = PlaybackRequestId;
        type Stream = FakeStream;

        fn connect(
            &self,
            request: PlaybackRequestId,
            _station: Station,
            _events: Sender<AudioEvent>,
        ) -> anyhow::Result<Self::Connected> {
            self.state.connects_entered.fetch_add(1, Ordering::SeqCst);
            self.state.connect_gate.wait();
            self.state.connects_finished.fetch_add(1, Ordering::SeqCst);
            // Read the behavior out before acting on it: panicking while the
            // guard is alive would poison the mutex for the rest of the test.
            let behavior = *self.state.behavior.lock().unwrap();
            match behavior {
                ConnectBehavior::Succeed => Ok(request),
                ConnectBehavior::Fail => Err(anyhow::anyhow!("fake connect failure")),
                ConnectBehavior::Panic => panic!("fake connect panic"),
            }
        }

        fn start(
            &self,
            request: PlaybackRequestId,
            _station: StationId,
            _connected: Self::Connected,
            _volume: &SharedVolume,
            _events: &Sender<AudioEvent>,
        ) -> anyhow::Result<(Self::Stream, SessionWorkers)> {
            if self.state.start_fails.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("fake output device failure"));
            }
            self.state.started.lock().unwrap().push(request);
            self.state.live_streams.fetch_add(1, Ordering::SeqCst);

            let stop = Arc::new(AtomicBool::new(false));
            let worker_state = Arc::clone(&self.state);
            worker_state.live_workers.fetch_add(1, Ordering::SeqCst);
            let worker = thread::spawn(move || {
                // Deliberately ignores `stop` until the gate opens: a worker
                // inside a blocking read cannot observe cancellation either.
                worker_state.worker_gate.wait();
                worker_state.live_workers.fetch_sub(1, Ordering::SeqCst);
            });

            Ok((
                FakeStream {
                    state: Arc::clone(&self.state),
                    _not_send: PhantomData,
                },
                SessionWorkers::new(stop, vec![worker]),
            ))
        }
    }

    /// A control loop under test, with the fake's gates in hand.
    struct Harness {
        command_tx: Sender<AudioCommand>,
        event_rx: Receiver<AudioEvent>,
        state: Arc<FakeState>,
        finished: Arc<Gate>,
        requests: PlaybackRequestSeq,
    }

    impl Harness {
        /// `connect_gate`/`worker_gate` open means "this step does not block".
        fn start(connect_gate: Gate, worker_gate: Gate) -> Self {
            let state = FakeState::new(connect_gate, worker_gate);
            let (command_tx, command_rx) = mpsc::channel();
            let (event_tx, event_rx) = mpsc::channel();
            let finished = Arc::new(Gate::new());

            let engine = FakeEngine {
                state: Arc::clone(&state),
            };
            let loop_finished = Arc::clone(&finished);
            thread::spawn(move || {
                run_control_loop(engine, command_rx, event_tx);
                loop_finished.open();
            });

            Self {
                command_tx,
                event_rx,
                state,
                finished,
                requests: PlaybackRequestSeq::new(),
            }
        }

        fn play(&self) -> PlaybackRequestId {
            let request = self.requests.next_id();
            self.command_tx
                .send(AudioCommand::Play {
                    request,
                    station: Box::new(station()),
                    volume: VolumePercent::clamped(50),
                })
                .unwrap();
            request
        }

        fn send(&self, command: AudioCommand) {
            self.command_tx.send(command).unwrap();
        }

        /// Wait for an event, failing with `what` if the control thread did not
        /// answer in time.
        fn expect_event(&self, what: &str) -> AudioEvent {
            match self.event_rx.recv_timeout(RESPONSIVE) {
                Ok(event) => event,
                Err(_) => panic!("control thread did not answer: {what}"),
            }
        }

        fn expect_loop_exit(&self) {
            assert!(
                wait_until(RESPONSIVE, || *self.finished.open.lock().unwrap()),
                "control loop did not exit while I/O was blocked"
            );
        }

        /// Release every blocking point so the fake's threads can exit.
        fn release(&self) {
            self.state.connect_gate.open();
            self.state.worker_gate.open();
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            // Never leave a failed test's fake threads parked on a gate.
            self.release();
        }
    }

    fn station() -> Station {
        Station {
            id: StationId::new("fake").unwrap(),
            name: StationName::new("fake").unwrap(),
            url: StreamUrl::parse("https://example.com/live.mp3").unwrap(),
            homepage: None,
            country: None,
            language: None,
            tags: vec![],
            codec: CodecKind::Mp3,
            bitrate: Some(BitrateKbps::new(128).unwrap()),
            votes: None,
            click_count: None,
            source: StationSource::RadioBrowser,
        }
    }

    #[test]
    fn stop_is_answered_while_a_connect_is_blocked() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        let request = harness.play();
        assert!(matches!(
            harness.expect_event("Connecting"),
            AudioEvent::Connecting { request: r, .. } if r == request
        ));

        harness.send(AudioCommand::Stop);

        assert_eq!(harness.expect_event("Stop"), AudioEvent::Stopped);
    }

    #[test]
    fn a_replacement_play_is_answered_while_the_previous_connect_is_blocked() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        let first = harness.play();
        assert!(matches!(
            harness.expect_event("Connecting"),
            AudioEvent::Connecting { request: r, .. } if r == first
        ));

        let second = harness.play();

        assert!(matches!(
            harness.expect_event("replacement Connecting"),
            AudioEvent::Connecting { request: r, .. } if r == second
        ));
    }

    #[test]
    fn shutdown_is_answered_while_a_connect_is_blocked() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        harness.play();
        harness.expect_event("Connecting");

        harness.send(AudioCommand::Shutdown);

        harness.expect_loop_exit();
    }

    #[test]
    fn stop_is_answered_while_a_session_worker_is_blocked() {
        let harness = Harness::start(Gate::opened(), Gate::new());
        harness.play();
        harness.expect_event("Connecting");
        assert!(matches!(
            harness.expect_event("Playing"),
            AudioEvent::Playing { .. }
        ));

        harness.send(AudioCommand::Stop);

        assert_eq!(harness.expect_event("Stop"), AudioEvent::Stopped);
        // The stream is released on the control thread even though its worker is
        // still parked, so the device is free immediately.
        assert_eq!(harness.state.live_streams(), 0);
    }

    #[test]
    fn shutdown_is_answered_while_a_session_worker_is_blocked() {
        let harness = Harness::start(Gate::opened(), Gate::new());
        harness.play();
        harness.expect_event("Connecting");
        harness.expect_event("Playing");

        harness.send(AudioCommand::Shutdown);

        harness.expect_loop_exit();
    }

    #[test]
    fn a_connect_that_completes_after_stop_never_starts_playback() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        harness.play();
        harness.expect_event("Connecting");
        harness.send(AudioCommand::Stop);
        assert_eq!(harness.expect_event("Stop"), AudioEvent::Stopped);

        // Let the superseded connect finish, then confirm it is discarded rather
        // than adopted: no session is started and no event is emitted for it.
        harness.state.connect_gate.open();
        assert!(wait_until(RECLAIM, || harness.state.connects_finished() == 1));
        assert!(
            !wait_until(Duration::from_millis(500), || !harness
                .state
                .started()
                .is_empty()),
            "a cancelled request must not start playback"
        );
        assert!(harness.event_rx.try_recv().is_err(), "no event after Stop");
    }

    #[test]
    fn a_connect_that_completes_after_being_replaced_never_starts_playback() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        let first = harness.play();
        harness.expect_event("Connecting");
        let second = harness.play();
        harness.expect_event("replacement Connecting");

        harness.state.connect_gate.open();
        assert!(wait_until(RECLAIM, || harness.state.connects_finished() == 2));

        assert_eq!(
            harness.expect_event("Playing"),
            AudioEvent::Playing {
                request: second,
                station: StationId::new("fake").unwrap(),
            }
        );
        assert_eq!(
            harness.state.started(),
            vec![second],
            "only the current request may start playback, never {first:?}"
        );
    }

    #[test]
    fn retired_session_workers_are_eventually_reclaimed() {
        let harness = Harness::start(Gate::opened(), Gate::new());
        harness.play();
        harness.expect_event("Connecting");
        harness.expect_event("Playing");
        harness.play();
        harness.expect_event("Connecting");
        harness.expect_event("Playing");
        harness.send(AudioCommand::Stop);
        assert_eq!(harness.expect_event("Stop"), AudioEvent::Stopped);

        // Both workers are still parked in their "blocking read"; teardown did
        // not wait for them.
        assert_eq!(harness.state.live_workers(), 2);
        assert_eq!(harness.state.live_streams(), 0);

        harness.state.worker_gate.open();
        assert!(
            wait_until(RECLAIM, || harness.state.live_workers() == 0),
            "retired workers were never reclaimed"
        );
    }

    #[test]
    fn rapid_play_bounds_connect_workers_and_only_the_latest_request_connects() {
        // Every station is wedged, so no connect worker can ever finish on its
        // own — exactly the case where unbounded spawning would pile up.
        let harness = Harness::start(Gate::new(), Gate::opened());
        let requests: Vec<_> = (0..MAX_CONNECT_WORKERS * 2 + 3)
            .map(|_| harness.play())
            .collect();

        // Every Play is still received and announced: the cap throttles work,
        // never command receipt.
        for expected in &requests {
            assert!(matches!(
                harness.expect_event("Connecting"),
                AudioEvent::Connecting { request, .. } if request == *expected
            ));
        }

        // The cap holds: uncancellable workers never exceed the ceiling.
        assert!(wait_until(RECLAIM, || harness.state.connects_entered()
            == MAX_CONNECT_WORKERS));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            harness.state.connects_entered(),
            MAX_CONNECT_WORKERS,
            "rapid Play must not spawn a connect worker per keypress"
        );

        // Releasing the wedged workers frees slots. The request the user settled
        // on is the one that connects; the ones it superseded are dropped.
        harness.state.connect_gate.open();
        let latest = *requests.last().unwrap();
        assert_eq!(
            harness.expect_event("Playing"),
            AudioEvent::Playing {
                request: latest,
                station: StationId::new("fake").unwrap(),
            }
        );
        assert_eq!(
            harness.state.started(),
            vec![latest],
            "only the latest request may start playback"
        );
        assert_eq!(
            harness.state.connects_entered(),
            MAX_CONNECT_WORKERS + 1,
            "superseded requests must not be connected once slots free"
        );
    }

    #[test]
    fn a_panicking_connect_worker_becomes_a_recoverable_failure() {
        // A panic backtrace on stderr is expected here; the panic is caught.
        let harness = Harness::start(Gate::opened(), Gate::opened());
        harness.state.set_behavior(ConnectBehavior::Panic);

        let request = harness.play();
        harness.expect_event("Connecting");

        assert!(
            matches!(
                harness.expect_event("Failed after connect panic"),
                AudioEvent::Failed { request: r, .. } if r == request
            ),
            "a panicking connect must surface as a recoverable failure"
        );

        // The connect slot and the pending request were both released, so the
        // runtime still plays afterwards rather than being wedged forever.
        harness.state.set_behavior(ConnectBehavior::Succeed);
        let next = harness.play();
        harness.expect_event("Connecting");
        assert_eq!(
            harness.expect_event("Playing after recovery"),
            AudioEvent::Playing {
                request: next,
                station: StationId::new("fake").unwrap(),
            }
        );
    }

    #[test]
    fn a_connect_error_becomes_a_recoverable_failure() {
        let harness = Harness::start(Gate::opened(), Gate::opened());
        harness.state.set_behavior(ConnectBehavior::Fail);

        let request = harness.play();
        harness.expect_event("Connecting");

        let event = harness.expect_event("Failed after connect error");
        assert!(matches!(
            &event,
            AudioEvent::Failed { request: r, message, .. }
                if *r == request && message.contains("fake connect failure")
        ));
        assert_eq!(harness.state.started(), Vec::new());
    }

    #[test]
    fn a_start_failure_becomes_a_recoverable_failure_without_leaking_workers() {
        let harness = Harness::start(Gate::opened(), Gate::opened());
        harness.state.start_fails.store(true, Ordering::SeqCst);

        let request = harness.play();
        harness.expect_event("Connecting");

        assert!(matches!(
            harness.expect_event("Failed after start error"),
            AudioEvent::Failed { request: r, .. } if r == request
        ));
        // A device failure must not leave a stream or a worker behind.
        assert_eq!(harness.state.live_streams(), 0);
        assert_eq!(harness.state.live_workers(), 0);
    }

    #[test]
    fn set_volume_is_answered_while_a_connect_is_blocked() {
        let harness = Harness::start(Gate::new(), Gate::opened());
        harness.play();
        harness.expect_event("Connecting");

        harness.send(AudioCommand::SetVolume(VolumePercent::clamped(30)));

        assert_eq!(
            harness.expect_event("SetVolume"),
            AudioEvent::VolumeChanged(VolumePercent::clamped(30))
        );
    }

    #[test]
    fn the_reaper_joins_retired_workers_without_blocking_the_retiring_thread() {
        let gate = Arc::new(Gate::new());
        let live = Arc::new(AtomicUsize::new(1));
        let worker_gate = Arc::clone(&gate);
        let worker_live = Arc::clone(&live);
        let worker = thread::spawn(move || {
            worker_gate.wait();
            worker_live.fetch_sub(1, Ordering::SeqCst);
        });

        let reaper = Reaper::spawn();
        let stop = Arc::new(AtomicBool::new(false));
        reaper.retire(SessionWorkers::new(Arc::clone(&stop), vec![worker]));

        // Retiring returned even though the worker is parked, and it cancelled.
        assert!(stop.load(Ordering::Relaxed), "retiring must cancel workers");
        assert_eq!(live.load(Ordering::SeqCst), 1);

        gate.open();
        reaper.join();
        assert_eq!(
            live.load(Ordering::SeqCst),
            0,
            "the reaper must join every retired worker"
        );
    }
}
