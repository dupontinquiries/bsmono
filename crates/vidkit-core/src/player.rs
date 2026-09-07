//! The pump loop.
//!
//! One concrete struct owning a source, a decoder, a queue, a presenter and a
//! clock. Not a trait: there is exactly one of these, and a `Player` trait
//! would be generality with nothing to vary.
//!
//! # The invariant this file exists to maintain
//!
//! Every recycle token gets back to the decoder, on every path. Frames leave
//! the decoder's pool through four exits — presented, dropped late, dropped
//! under pressure, discarded on flush — and three of those are easy to forget.
//! A leak here does not fail a test on software decode (tokens are `None`); it
//! shows up as playback freezing after ten minutes on a Fairphone. So the
//! tests below count buffers rather than trusting the code to be careful.
//!
//! # Bounded work
//!
//! `tick` does a fixed maximum of decode work and then returns, even if more
//! is available. The alternative — loop until nothing is left — makes a single
//! tick take arbitrarily long after a seek, and on a phone that is a visible
//! stall in the UI thread. The budget is the steerable knob: raise it for
//! throughput, lower it for responsiveness.

use crate::clock::SyncPolicy;
use crate::frame::RecycleToken;
use crate::queue::{FrameQueue, Poll as QueuePoll, Pressure};
use crate::traits::{Clock, DecodeOutput, Decoder, Error, MediaSource, Packet, Presenter, Result};
use std::time::Duration;

/// What a tick did. One event per tick, so the caller can act on it without
/// unpacking a batch.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// A frame went to the presenter.
    Presented { pts: Duration },
    /// The decoder reconfigured. The queue has been flushed and the presenter
    /// released; resize presenter resources before the next tick.
    FormatChanged { width: u32, height: u32 },
    /// Everything is decoded and shown.
    EndOfStream,
    /// The media URL expired. Re-resolve it on the source and keep ticking —
    /// this is routine on YouTube and is deliberately not an error, because
    /// tearing down and rebuilding the decoder for it would drop the buffer
    /// pool and cause a visible stall.
    NeedsReresolve,
    /// Nothing to do. `wake_in` is how long the caller may sleep — `None`
    /// means the queue is starved and the caller should tick again as soon as
    /// more input is available rather than sleeping.
    Idle { wake_in: Option<Duration> },
}

#[derive(Copy, Clone, Debug)]
pub struct Budget {
    /// Packets offered to the decoder per tick.
    pub packets_per_tick: u32,
    /// Frames drained from the decoder per tick.
    pub frames_per_tick: u32,
}

impl Default for Budget {
    fn default() -> Self {
        // Enough to keep a 60 fps stream fed from a ~120 Hz tick without
        // letting one tick run away after a seek.
        Self {
            packets_per_tick: 8,
            frames_per_tick: 4,
        }
    }
}

pub struct Player {
    source: Box<dyn MediaSource>,
    decoder: Box<dyn Decoder>,
    presenter: Box<dyn Presenter>,
    clock: Box<dyn Clock>,
    queue: FrameQueue,
    budget: Budget,
    /// A packet the decoder refused. Held and retried; dropping it would
    /// corrupt the bitstream.
    pending: Option<Packet>,
    /// The source has no more packets. Distinct from the decoder being done.
    source_drained: bool,
    drain_requested: bool,
    ended: bool,
}

impl Player {
    pub fn new(
        source: Box<dyn MediaSource>,
        decoder: Box<dyn Decoder>,
        presenter: Box<dyn Presenter>,
        clock: Box<dyn Clock>,
        refresh_hz: u32,
        queue_capacity: usize,
    ) -> Self {
        Self {
            source,
            decoder,
            presenter,
            clock,
            queue: FrameQueue::new(queue_capacity, SyncPolicy::for_refresh_hz(refresh_hz))
                .with_pressure(Pressure::Normal),
            budget: Budget::default(),
            pending: None,
            source_drained: false,
            drain_requested: false,
            ended: false,
        }
    }

    pub fn with_budget(mut self, b: Budget) -> Self {
        self.budget = b;
        self
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn clock_mut(&mut self) -> &mut dyn Clock {
        self.clock.as_mut()
    }

    /// Hand a batch of tokens back. Isolated so every call site is one line
    /// and none of them can forget the loop.
    fn give_back(&mut self, tokens: Vec<RecycleToken>) -> Result<()> {
        for t in tokens {
            self.decoder.recycle(t)?;
        }
        Ok(())
    }

    /// Reclaim whatever the queue discarded since last time.
    fn reclaim_queue(&mut self) -> Result<()> {
        let tokens = self.queue.take_reclaimed();
        self.give_back(tokens)
    }

    fn feed(&mut self) -> Result<()> {
        for _ in 0..self.budget.packets_per_tick {
            if self.source_drained {
                return Ok(());
            }
            // Take the held packet, or pull a new one.
            let pkt = match self.pending.take() {
                Some(p) => p,
                None => match self.source.next_packet() {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        self.source_drained = true;
                        return Ok(());
                    }
                    Err(Error::NeedsReresolve) => return Err(Error::NeedsReresolve),
                    Err(e) => return Err(e),
                },
            };

            match self.decoder.send_packet(&pkt) {
                // Refused: put it back and stop feeding this tick. The *same*
                // packet is retried next time.
                Ok(false) => {
                    self.pending = Some(pkt);
                    return Ok(());
                }
                Ok(true) => {}
                Err(e) => {
                    // Keep the packet so a recoverable error does not eat it.
                    self.pending = Some(pkt);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Drain the decoder into the queue. Returns a format change if one
    /// happened, since that must interrupt the tick.
    fn drain_decoder(&mut self) -> Result<Option<Event>> {
        for _ in 0..self.budget.frames_per_tick {
            if self.queue.is_full() {
                return Ok(None);
            }
            match self.decoder.receive_frame()? {
                DecodeOutput::Frame(f) => {
                    if let Some(returned) = self.queue.push(f) {
                        // Defensive, and currently unreachable: the `is_full`
                        // check above runs immediately before this push and
                        // nothing else pushes to the queue. Mutation testing
                        // confirms it — deleting the recycle here kills no
                        // test, because the branch never executes.
                        //
                        // Kept rather than deleted because `push` returning the
                        // frame is part of its contract, and the day a second
                        // producer appears (a second decoder for a picture-in-
                        // picture stream, say) this is the difference between a
                        // lost picture and a leaked buffer. Marked as a
                        // justified survivor in tools/mutate.py so it does not
                        // read as an untested gap.
                        if let Some(t) = returned.recycle {
                            self.decoder.recycle(t)?;
                        }
                        return Ok(None);
                    }
                }
                DecodeOutput::FormatChanged { width, height } => {
                    // Everything queued belongs to the old geometry.
                    self.queue.flush();
                    self.reclaim_queue()?;
                    let released = self.presenter.release_all();
                    self.give_back(released)?;
                    return Ok(Some(Event::FormatChanged { width, height }));
                }
                DecodeOutput::EndOfStream => {
                    self.ended = true;
                    return Ok(None);
                }
                DecodeOutput::Idle => return Ok(None),
            }
        }
        Ok(None)
    }

    /// One step. Call in a loop, sleeping for `wake_in` when idle.
    pub fn tick(&mut self) -> Result<Event> {
        self.reclaim_queue()?;

        match self.feed() {
            Ok(()) => {}
            Err(Error::NeedsReresolve) => return Ok(Event::NeedsReresolve),
            Err(e) => return Err(e),
        }

        // Once the source is empty, ask the decoder to flush out its reordering
        // window. Without this the last few frames of every video are lost.
        if self.source_drained && self.pending.is_none() && !self.drain_requested {
            self.drain_requested = true;
        }

        if let Some(ev) = self.drain_decoder()? {
            return Ok(ev);
        }

        match self.queue.poll(self.clock.now()) {
            QueuePoll::Show(frame) => {
                let pts = frame.pts;
                let released = self.presenter.submit(frame)?;
                if let Some(t) = released {
                    self.decoder.recycle(t)?;
                }
                self.reclaim_queue()?;
                Ok(Event::Presented { pts })
            }
            QueuePoll::Wait(d) => {
                self.reclaim_queue()?;
                Ok(Event::Idle { wake_in: Some(d) })
            }
            QueuePoll::Starved => {
                self.reclaim_queue()?;
                if self.ended && self.source_drained {
                    // Let go of the last displayed frame only at the very end;
                    // releasing it earlier would blank the screen between the
                    // final frame and EOS.
                    let released = self.presenter.release_all();
                    self.give_back(released)?;
                    Ok(Event::EndOfStream)
                } else {
                    Ok(Event::Idle { wake_in: None })
                }
            }
        }
    }

    /// Seek. Order matters: stop handing out buffers before flushing the
    /// decoder, or the decoder frees a pool that the presenter still points at.
    pub fn seek(&mut self, to: Duration) -> Result<()> {
        let released = self.presenter.release_all();
        self.give_back(released)?;
        self.queue.flush();
        self.reclaim_queue()?;

        self.decoder.flush()?;
        self.source.seek(to)?;

        self.pending = None;
        self.source_drained = false;
        self.drain_requested = false;
        self.ended = false;
        Ok(())
    }

    /// After the caller has re-resolved an expired URL on the source.
    pub fn resume_after_reresolve(&mut self) {
        self.source_drained = false;
    }
}
