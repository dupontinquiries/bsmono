//! engine — one worker thread, one inbox, one outbox, one rule.
//!
//! The rule: **only the newest request matters.** The worker drains its inbox
//! before running anything, keeping the last request (and the last
//! reconfiguration); the UI thread never blocks — it `poll()`s outcomes from
//! its frame loop. Slow providers therefore cost one in-flight completion of
//! latency, never a queue of them, and a provider swap (the loop pane's
//! host/model fields) takes effect on the very next request.
//!
//! Threading is std channels only. Dropping the [`Engine`] closes the inbox;
//! the worker sees the disconnect and exits after its current call.

use crate::{CompletionOutcome, CompletionRequest, Provider};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::Instant;

enum Msg {
    Req(CompletionRequest),
    Swap(Box<dyn Provider>),
}

pub struct Engine {
    tx: Sender<Msg>,
    rx: Receiver<CompletionOutcome>,
}

impl Engine {
    pub fn new(provider: Box<dyn Provider>) -> Engine {
        let (tx, worker_rx) = channel::<Msg>();
        let (out_tx, rx) = channel::<CompletionOutcome>();
        std::thread::Builder::new()
            .name("aperture-complete".into())
            .spawn(move || worker(provider, worker_rx, out_tx))
            .expect("spawn completion worker");
        Engine { tx, rx }
    }

    /// Queue a request. Never blocks; superseded requests are coalesced away.
    pub fn request(&self, req: CompletionRequest) {
        let _ = self.tx.send(Msg::Req(req));
    }

    /// Replace the provider. Applies before the next request runs.
    pub fn swap_provider(&self, provider: Box<dyn Provider>) {
        let _ = self.tx.send(Msg::Swap(provider));
    }

    /// Non-blocking; call once per frame.
    pub fn poll(&self) -> Option<CompletionOutcome> {
        self.rx.try_recv().ok()
    }
}

fn worker(mut provider: Box<dyn Provider>, rx: Receiver<Msg>, tx: Sender<CompletionOutcome>) {
    // A message pulled mid-stream by `should_cancel` (see below) carries
    // over to the NEXT iteration instead of being lost — it flows through
    // exactly the same apply/drain logic as an ordinary `rx.recv()` message,
    // so a cancel-then-coalesce is indistinguishable from the normal path.
    let mut primed: Option<Msg> = None;
    loop {
        let first = match primed.take() {
            Some(m) => m,
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => return,
            },
        };
        let mut req: Option<CompletionRequest> = None;
        let apply = |m: Msg, req: &mut Option<CompletionRequest>, provider: &mut Box<dyn Provider>| match m {
            Msg::Req(r) => *req = Some(r),
            Msg::Swap(p) => *provider = p,
        };
        apply(first, &mut req, &mut provider);
        loop {
            match rx.try_recv() {
                Ok(m) => apply(m, &mut req, &mut provider),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let Some(req) = req else { continue };
        let t0 = Instant::now();
        let name = provider.name().to_string();
        let gen = req.generation;

        // `should_cancel` checks the inbox WITHOUT blocking; if something
        // arrived, that message becomes `primed` for the next loop turn
        // (this closure's only job) and streaming stops — the provider
        // drops its connection on return, which for a TCP stream means the
        // socket closes, actually abandoning the in-flight request rather
        // than just discarding its eventual answer.
        let mut caught: Option<Msg> = None;
        let mut should_cancel = || match rx.try_recv() {
            Ok(m) => {
                caught = Some(m);
                true
            }
            Err(_) => false,
        };
        // Providers report full-text-so-far outcomes, not raw fragments —
        // that's what `ghost.rs` already knows how to paint (replace, not
        // append), so streaming needed no UI-side protocol change beyond
        // `is_final`. Every delta is `is_final: false`; the close-out below
        // is always the one `is_final: true` outcome for this generation,
        // sent exactly once regardless of how many deltas preceded it —
        // even zero, for a non-streaming provider's default impl.
        let mut acc = String::new();
        let mut on_delta = |delta: &str| {
            acc.push_str(delta);
            let _ = tx.send(CompletionOutcome {
                generation: gen,
                provider: name.clone(),
                latency_ms: t0.elapsed().as_millis() as u64,
                result: Ok(acc.clone()),
                is_final: false,
            });
        };
        let result = provider.complete_streaming(&req, &mut on_delta, &mut should_cancel);
        let outcome = CompletionOutcome {
            generation: gen,
            provider: name,
            latency_ms: t0.elapsed().as_millis() as u64,
            result,
            is_final: true,
        };
        if tx.send(outcome).is_err() {
            return; // UI gone
        }
        primed = caught;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel as sync_channel, Receiver as R, Sender as S};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A provider that blocks until the test releases it — makes coalescing
    /// deterministic without sleeping-and-hoping.
    struct Gated {
        gate: Mutex<R<()>>,
        seen: S<u64>,
    }
    impl Provider for Gated {
        fn name(&self) -> &str {
            "gated"
        }
        fn complete(&mut self, req: &CompletionRequest) -> Result<String, String> {
            // Announce entry BEFORE blocking: the test uses this to know the
            // worker is parked inside this call while it enqueues more.
            let _ = self.seen.send(req.generation);
            self.gate.lock().unwrap().recv().ok();
            Ok(format!("done-{}", req.generation))
        }
    }

    fn req(generation: u64) -> CompletionRequest {
        CompletionRequest { generation, prefix: String::new(), suffix: String::new(), lang: "rust".into() }
    }

    #[test]
    fn stale_requests_coalesce_to_the_newest() {
        let (open, gate) = sync_channel::<()>();
        let (seen_tx, seen) = sync_channel::<u64>();
        let eng = Engine::new(Box::new(Gated { gate: Mutex::new(gate), seen: seen_tx }));

        eng.request(req(1));
        // Worker is now inside complete(1), parked on the gate…
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        // …so these three pile up in its inbox.
        eng.request(req(2));
        eng.request(req(3));
        eng.request(req(4));
        open.send(()).unwrap(); // finish gen 1 → worker drains {2,3,4}, runs only 4
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), 4);
        open.send(()).unwrap(); // finish gen 4
        assert!(seen.try_recv().is_err(), "generations 2 and 3 never ran");

        // Outcomes arrived for exactly the generations that ran.
        let mut gens = Vec::new();
        let t0 = Instant::now();
        while gens.len() < 2 && t0.elapsed() < Duration::from_secs(5) {
            if let Some(o) = eng.poll() {
                gens.push(o.generation);
            }
        }
        assert_eq!(gens, vec![1, 4]);
    }

    #[test]
    fn swap_applies_before_next_request() {
        struct Named(&'static str);
        impl Provider for Named {
            fn name(&self) -> &str {
                self.0
            }
            fn complete(&mut self, _: &CompletionRequest) -> Result<String, String> {
                Ok(self.0.into())
            }
        }
        let eng = Engine::new(Box::new(Named("first")));
        eng.swap_provider(Box::new(Named("second")));
        eng.request(req(9));
        let t0 = Instant::now();
        let out = loop {
            if let Some(o) = eng.poll() {
                break o;
            }
            assert!(t0.elapsed() < Duration::from_secs(5), "worker never answered");
            std::thread::yield_now();
        };
        assert_eq!(out.provider, "second");
        assert_eq!(out.result.unwrap(), "second");
    }
    /// A provider whose `complete_streaming` actually streams: sends
    /// fragments one at a time, checking `should_cancel` between each and
    /// announcing its progress on `seen` so a test can synchronize without
    /// sleeping-and-hoping. `gate` (if `Some`) blocks before each fragment,
    /// giving a test a moment to enqueue a superseding request mid-stream.
    /// `gate_budget`: gating is a GLOBAL step count, not per-call — a
    /// superseded generation gets re-invoked from `complete_streaming`'s
    /// top when the worker picks up the primed message, and its fragment
    /// loop starts over from index 0. A per-call gate would then block
    /// forever waiting for a release the test only ever sends a fixed
    /// number of times in total; a shared budget across every call lets
    /// exactly the first N fragment-boundaries block, however many
    /// `complete_streaming` invocations they land across.
    struct Streamer {
        fragments: Vec<&'static str>,
        seen: S<String>,
        gate: Option<(Mutex<R<()>>, Mutex<usize>)>,
    }
    impl Provider for Streamer {
        fn name(&self) -> &str {
            "streamer"
        }
        fn complete(&mut self, _req: &CompletionRequest) -> Result<String, String> {
            Ok(self.fragments.concat())
        }
        fn complete_streaming(
            &mut self,
            _req: &CompletionRequest,
            on_delta: &mut dyn FnMut(&str),
            should_cancel: &mut dyn FnMut() -> bool,
        ) -> Result<String, String> {
            let mut acc = String::new();
            for frag in &self.fragments {
                if let Some((gate, budget)) = &self.gate {
                    let should_gate = {
                        let mut b = budget.lock().unwrap();
                        if *b > 0 {
                            *b -= 1;
                            true
                        } else {
                            false
                        }
                    };
                    if should_gate {
                        let _ = self.seen.send(acc.clone());
                        gate.lock().unwrap().recv().ok();
                    }
                }
                if should_cancel() {
                    return Ok(acc); // partial — the caller's generation is stale anyway
                }
                acc.push_str(frag);
                on_delta(frag);
            }
            Ok(acc)
        }
    }

    #[test]
    fn streaming_delivers_growing_partials_then_exactly_one_final() {
        let (seen_tx, _seen) = sync_channel::<String>();
        let eng = Engine::new(Box::new(Streamer {
            fragments: vec!["fn ", "main", "() {}"],
            seen: seen_tx,
            gate: None,
        }));
        eng.request(req(1));
        let t0 = Instant::now();
        let mut outcomes = Vec::new();
        while outcomes.len() < 4 && t0.elapsed() < Duration::from_secs(5) {
            if let Some(o) = eng.poll() {
                outcomes.push(o);
            }
        }
        let texts: Vec<String> = outcomes.iter().map(|o| o.result.clone().unwrap()).collect();
        assert_eq!(
            texts,
            vec!["fn ", "fn main", "fn main() {}", "fn main() {}"],
            "three growing partials, then the close-out repeating the last state"
        );
        assert_eq!(
            outcomes.iter().map(|o| o.is_final).collect::<Vec<_>>(),
            vec![false, false, false, true],
            "exactly the LAST outcome is marked final — a consumer filtering on is_final sees one row per generation, not four"
        );
        assert!(outcomes.iter().all(|o| o.generation == 1));
        std::thread::sleep(Duration::from_millis(50));
        assert!(eng.poll().is_none(), "nothing trails after the final outcome");
    }

    #[test]
    fn a_newer_request_cancels_an_in_flight_stream_mid_fragment() {
        let (open, gate) = sync_channel::<()>();
        let (seen_tx, seen) = sync_channel::<String>();
        let eng = Engine::new(Box::new(Streamer {
            fragments: vec!["a", "b", "c", "d", "e"],
            seen: seen_tx,
            gate: Some((Mutex::new(gate), Mutex::new(2))), // exactly the two releases this test sends
        }));
        eng.request(req(1));
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), "", "parked before the first fragment");
        open.send(()).unwrap(); // let it check should_cancel and (finding nothing) emit "a"
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), "a", "parked before the second fragment");

        // A newer request arrives WHILE the stream is mid-flight.
        eng.request(req(2));
        open.send(()).unwrap(); // release the gate: should_cancel now finds gen 2 waiting

        // The worker abandons generation 1 (never reaches "abcde") and
        // starts generation 2 instead — proving the cancellation actually
        // stopped the stream rather than merely being polled and ignored.
        let t0 = Instant::now();
        let mut saw_gen2 = false;
        while t0.elapsed() < Duration::from_secs(5) {
            if let Some(o) = eng.poll() {
                if o.generation == 2 {
                    saw_gen2 = true;
                    break;
                }
                assert_ne!(o.result.as_deref(), Ok("abcde"), "generation 1 must not run to completion after being superseded");
            }
        }
        assert!(saw_gen2, "generation 2 ran after cancelling generation 1 mid-stream");
    }

}
