//! Places for work that keeps a whole file in memory or a connection open,
//! counted for each MCP so that one cannot use up the instance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct LimitedPlaces {
    max: usize,
    taken: Arc<Mutex<HashMap<i64, usize>>>,
}

/// One of an MCP's places. Dropping it gives the place back.
#[derive(Debug)]
pub struct Place {
    mcp_id: i64,
    taken: Arc<Mutex<HashMap<i64, usize>>>,
}

impl LimitedPlaces {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            taken: Arc::default(),
        }
    }

    /// Take one of the MCP's places, or `None` when all are taken.
    pub fn take(&self, mcp_id: i64) -> Option<Place> {
        let mut taken = self
            .taken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let running = taken.entry(mcp_id).or_insert(0);
        if *running >= self.max {
            return None;
        }
        *running += 1;
        Some(Place {
            mcp_id,
            taken: self.taken.clone(),
        })
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut taken = self
            .taken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match taken.get_mut(&self.mcp_id) {
            Some(running) if *running > 1 => *running -= 1,
            _ => {
                taken.remove(&self.mcp_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_places_for_each_mcp() {
        let places = LimitedPlaces::new(2);
        let first = places.take(7).unwrap();
        let _second = places.take(7).unwrap();
        assert!(places.take(7).is_none());
        assert!(places.take(8).is_some(), "another MCP has its own places");

        drop(first);
        let _third = places.take(7).unwrap();
        assert!(places.take(7).is_none());
    }
}
