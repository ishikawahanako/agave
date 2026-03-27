use {
    log::info,
    std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Instant,
    },
};

/// Tracks the first shred arrival time per slot and computes latency
/// to various Geyser notification delivery points.
pub struct LatencyTracker {
    inner: Arc<Mutex<LatencyTrackerInner>>,
}

struct LatencyTrackerInner {
    /// First shred arrival time per slot
    first_shred_times: HashMap<u64, Instant>,
    /// Accumulated latency samples for periodic reporting
    samples: LatencySamples,
    /// Last report time
    last_report: Instant,
}

struct LatencySamples {
    slot_status_us: Vec<u64>,
    transaction_us: Vec<u64>,
    account_us: Vec<u64>,
    entry_us: Vec<u64>,
    block_us: Vec<u64>,
}

impl LatencySamples {
    fn new() -> Self {
        Self {
            slot_status_us: Vec::new(),
            transaction_us: Vec::new(),
            account_us: Vec::new(),
            entry_us: Vec::new(),
            block_us: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.slot_status_us.clear();
        self.transaction_us.clear();
        self.account_us.clear();
        self.entry_us.clear();
        self.block_us.clear();
    }
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LatencyTrackerInner {
                first_shred_times: HashMap::new(),
                samples: LatencySamples::new(),
                last_report: Instant::now(),
            })),
        }
    }

    /// Record the arrival of the first shred for a slot.
    /// Called when we see SlotStatus::FirstShredReceived from the Geyser pipeline.
    pub fn record_first_shred(&self, slot: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.first_shred_times.entry(slot).or_insert_with(Instant::now);
    }

    /// Get the latency in microseconds from first shred arrival to now for a given slot.
    /// Returns None if we haven't seen the first shred for this slot.
    fn latency_us(&self, slot: u64) -> Option<u64> {
        let inner = self.inner.lock().unwrap();
        inner
            .first_shred_times
            .get(&slot)
            .map(|t| t.elapsed().as_micros() as u64)
    }

    /// Record a slot status notification latency
    pub fn record_slot_status(&self, slot: u64) {
        if let Some(us) = self.latency_us(slot) {
            self.inner.lock().unwrap().samples.slot_status_us.push(us);
            self.maybe_report();
        }
    }

    /// Record a transaction notification latency
    pub fn record_transaction(&self, slot: u64) {
        if let Some(us) = self.latency_us(slot) {
            self.inner.lock().unwrap().samples.transaction_us.push(us);
        }
    }

    /// Record an account notification latency
    pub fn record_account(&self, slot: u64) {
        if let Some(us) = self.latency_us(slot) {
            self.inner.lock().unwrap().samples.account_us.push(us);
        }
    }

    /// Record an entry notification latency
    pub fn record_entry(&self, slot: u64) {
        if let Some(us) = self.latency_us(slot) {
            self.inner.lock().unwrap().samples.entry_us.push(us);
        }
    }

    /// Record a block metadata notification latency
    pub fn record_block(&self, slot: u64) {
        if let Some(us) = self.latency_us(slot) {
            self.inner.lock().unwrap().samples.block_us.push(us);
        }
    }

    /// Clean up old slot entries to prevent unbounded memory growth.
    /// Call periodically (e.g. when a slot is rooted).
    pub fn cleanup_old_slots(&self, rooted_slot: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .first_shred_times
            .retain(|&slot, _| slot >= rooted_slot.saturating_sub(100));
    }

    /// Check if it's time to report and do so
    fn maybe_report(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.last_report.elapsed().as_secs() >= 10 {
            Self::report_inner(&inner.samples);
            inner.samples.clear();
            inner.last_report = Instant::now();
        }
    }

    fn report_inner(samples: &LatencySamples) {
        if samples.slot_status_us.is_empty()
            && samples.transaction_us.is_empty()
            && samples.account_us.is_empty()
            && samples.entry_us.is_empty()
            && samples.block_us.is_empty()
        {
            return;
        }

        let format_stats = |name: &str, data: &[u64]| {
            if data.is_empty() {
                return format!("{name}: no samples");
            }
            let mut sorted = data.to_vec();
            sorted.sort_unstable();
            let len = sorted.len();
            let p50 = sorted[len / 2];
            let p99 = sorted[(len * 99) / 100.max(1)];
            let max = sorted[len - 1];
            let avg: u64 = sorted.iter().sum::<u64>() / len as u64;
            format!(
                "{name}: n={len} avg={avg}us p50={p50}us p99={p99}us max={max}us"
            )
        };

        info!(
            "shred-to-geyser latency: {} | {} | {} | {} | {}",
            format_stats("slot", &samples.slot_status_us),
            format_stats("tx", &samples.transaction_us),
            format_stats("account", &samples.account_us),
            format_stats("entry", &samples.entry_us),
            format_stats("block", &samples.block_us),
        );
    }
}

impl Clone for LatencyTracker {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
