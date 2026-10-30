//! Holds the answer of one request of a provider until the test lets it go, so
//! that the test decides what happens while the request is in flight. The
//! answer is computed on a thread of the runtime, which waits: the tests that
//! use it run on more than one.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

#[derive(Default)]
pub struct Gate {
    armed: Mutex<Option<String>>,
    arrived: Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    /// Hold the next request that calls [`hold`](Self::hold) with `key`.
    pub fn arm(&self, key: &str) {
        *self.released.lock().unwrap() = false;
        *self.armed.lock().unwrap() = Some(key.to_owned());
    }

    /// Called by the provider where a request may be held: when `key` is the
    /// one armed, tells the test the request is there, then holds it until
    /// [`release`](Self::release).
    pub fn hold(&self, key: &str) {
        {
            let mut armed = self.armed.lock().unwrap();
            if armed.as_deref() != Some(key) {
                return;
            }
            *armed = None;
        }
        self.arrived.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }

    /// The held request is at the provider, which has not answered.
    pub async fn reached(&self) {
        tokio::time::timeout(Duration::from_secs(20), self.arrived.notified())
            .await
            .expect("the held request reached the provider");
    }

    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
