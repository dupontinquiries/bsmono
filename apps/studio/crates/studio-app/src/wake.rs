//! wake — background workers wake the window when they have news.
//!
//! Twice now a worker finished and the window never showed it: the video-audio
//! countdown sat at "1 to go", and a touch-up that had LANDED still read
//! "measuring" four minutes later -- both until the mouse moved. They relied on
//! the window polling itself on a timer (`request_repaint_after`), which did
//! not reliably fire. A worker that sends through this channel wakes the
//! window after every message instead, so news is shown when it arrives.
//!
//! The sender has the same `send` as `mpsc::Sender`, so a worker's body does
//! not change -- only how its channel is made.

use std::sync::{mpsc, Arc};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A wake that does nothing: for tests and headless use.
pub fn none() -> Wake { Arc::new(|| {}) }

pub struct Sender<T> { tx: mpsc::Sender<T>, wake: Wake }

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self { Sender { tx: self.tx.clone(), wake: self.wake.clone() } }
}

impl<T> Sender<T> {
    pub fn send(&self, v: T) -> Result<(), mpsc::SendError<T>> {
        let r = self.tx.send(v);
        (self.wake)();
        r
    }
}

/// A channel whose sends wake the window.
pub fn channel<T>(wake: Wake) -> (Sender<T>, mpsc::Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Sender { tx, wake }, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_send_wakes_the_window_even_from_another_thread() {
        let n = Arc::new(AtomicUsize::new(0));
        let k = n.clone();
        let (tx, rx) = channel::<u32>(Arc::new(move || { k.fetch_add(1, Ordering::SeqCst); }));
        let t = tx.clone();
        std::thread::spawn(move || { for i in 0..5 { t.send(i).unwrap(); } }).join().unwrap();
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        assert_eq!(n.load(Ordering::SeqCst), 5, "one wake per message");
        drop(rx);
        assert!(tx.send(9).is_err(), "a closed window is still reported, and still woken harmlessly");
    }
}
