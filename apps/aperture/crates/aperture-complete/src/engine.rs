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
    // Block for the next message, then drain everything queued behind it:
    // the last Req wins, the last Swap wins.
    while let Ok(first) = rx.recv() {
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
        let result = provider.complete(&req);
        let outcome = CompletionOutcome {
            generation: req.generation,
            provider: provider.name().to_string(),
            latency_ms: t0.elapsed().as_millis() as u64,
            result,
        };
        if tx.send(outcome).is_err() {
            return; // UI gone
        }
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
}
