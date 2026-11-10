//! Snowflake id generator, mirrors `backend/utils/snowflake.py`.
//!
//! Layout (identical to the Python implementation):
//!
//! ```text
//! 1 bit unused | 41 bits timestamp | 5 bits datacenter | 5 bits worker | 12 bits sequence
//! ```
//!
//! The Twitter epoch is `1288834974657` (2010-11-04), worker id defaults to `2`
//! and datacenter id to `1`, exactly like the Python defaults, so ids generated
//! by both implementations are interchangeable.

use std::sync::Mutex;
use std::sync::OnceLock;

use crate::exception::{ApiError, ApiResult};

const WORKER_ID_BITS: u8 = 5;
const DATACENTER_ID_BITS: u8 = 5;
const SEQUENCE_BITS: u8 = 12;

const MAX_WORKER_ID: u64 = (1 << WORKER_ID_BITS) - 1;
const MAX_DATACENTER_ID: u64 = (1 << DATACENTER_ID_BITS) - 1;

const WORKER_ID_SHIFT: u8 = SEQUENCE_BITS;
const DATACENTER_ID_SHIFT: u8 = SEQUENCE_BITS + WORKER_ID_BITS;
const TIMESTAMP_LEFT_SHIFT: u8 = SEQUENCE_BITS + WORKER_ID_BITS + DATACENTER_ID_BITS;
const SEQUENCE_MASK: u64 = (1 << SEQUENCE_BITS) - 1;

/// Twitter epoch, in milliseconds.
pub const TWEPOCH: i64 = 1_288_834_974_657;

/// Snowflake id worker.
#[derive(Debug)]
pub struct IdWorker {
    worker_id: u64,
    datacenter_id: u64,
    state: Mutex<WorkerState>,
}

#[derive(Debug, Default)]
struct WorkerState {
    last_timestamp: i64,
    sequence: u64,
}

impl IdWorker {
    /// Create a worker. Fails when the ids are out of range.
    pub fn new(datacenter_id: u64, worker_id: u64) -> anyhow::Result<Self> {
        if worker_id > MAX_WORKER_ID {
            return Err(anyhow::anyhow!(
                "worker_id {worker_id} is out of range (0..={MAX_WORKER_ID})"
            ));
        }
        if datacenter_id > MAX_DATACENTER_ID {
            return Err(anyhow::anyhow!(
                "datacenter_id {datacenter_id} is out of range (0..={MAX_DATACENTER_ID})"
            ));
        }
        Ok(Self {
            worker_id,
            datacenter_id,
            state: Mutex::new(WorkerState {
                last_timestamp: -1,
                sequence: 0,
            }),
        })
    }

    /// Next id. Fails when the system clock moved backwards, like
    /// `InvalidSystemClock` in the Python implementation.
    pub fn next_id(&self) -> ApiResult<i64> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApiError::internal("snowflake worker lock poisoned"))?;

        let mut timestamp = now_millis();
        if timestamp < state.last_timestamp {
            return Err(ApiError::internal(format!(
                "clock is moving backwards: refusing to generate ids until {}",
                state.last_timestamp
            )));
        }

        if timestamp == state.last_timestamp {
            state.sequence = (state.sequence + 1) & SEQUENCE_MASK;
            if state.sequence == 0 {
                timestamp = wait_next_millis(state.last_timestamp);
            }
        } else {
            state.sequence = 0;
        }
        state.last_timestamp = timestamp;

        let id = ((timestamp - TWEPOCH) << TIMESTAMP_LEFT_SHIFT)
            | ((self.datacenter_id << DATACENTER_ID_SHIFT) as i64)
            | ((self.worker_id << WORKER_ID_SHIFT) as i64)
            | (state.sequence as i64);
        Ok(id)
    }

    /// Worker id.
    pub fn worker_id(&self) -> u64 {
        self.worker_id
    }

    /// Datacenter id.
    pub fn datacenter_id(&self) -> u64 {
        self.datacenter_id
    }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn wait_next_millis(last_timestamp: i64) -> i64 {
    let mut timestamp = now_millis();
    while timestamp <= last_timestamp {
        std::hint::spin_loop();
        timestamp = now_millis();
    }
    timestamp
}

/// Process wide worker, configured from `server.snowflake.*` at startup.
static GLOBAL: OnceLock<IdWorker> = OnceLock::new();

/// Configure the process wide worker once, at startup.
///
/// The defaults match the Python implementation (`datacenter_id=1`,
/// `worker_id=2`).
pub fn init(datacenter_id: u64, worker_id: u64) -> anyhow::Result<()> {
    let worker = IdWorker::new(datacenter_id, worker_id)?;
    if GLOBAL.set(worker).is_err() {
        tracing::warn!("snowflake worker already initialized, keeping the first configuration");
    }
    Ok(())
}

/// The process wide worker, initialized with the Python defaults on first use.
pub fn worker() -> &'static IdWorker {
    GLOBAL.get_or_init(|| IdWorker::new(1, 2).expect("the default snowflake ids are always valid"))
}

/// Generate a new id with the process wide worker.
pub fn next_id() -> ApiResult<i64> {
    worker().next_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let worker = IdWorker::new(1, 2).unwrap();
        let mut previous = 0;
        for _ in 0..5_000 {
            let id = worker.next_id().unwrap();
            assert!(id > previous, "ids must increase: {id} <= {previous}");
            assert!(id > 0);
            previous = id;
        }
    }

    #[test]
    fn bits_are_laid_out_like_the_python_worker() {
        let worker = IdWorker::new(1, 2).unwrap();
        let id = worker.next_id().unwrap();
        // worker id occupies bits 12..17, datacenter id bits 17..22
        assert_eq!((id >> WORKER_ID_SHIFT) & (MAX_WORKER_ID as i64), 2);
        assert_eq!((id >> DATACENTER_ID_SHIFT) & (MAX_DATACENTER_ID as i64), 1);
    }

    #[test]
    fn out_of_range_ids_are_rejected() {
        assert!(IdWorker::new(0, 32).is_err());
        assert!(IdWorker::new(32, 0).is_err());
        assert!(IdWorker::new(31, 31).is_ok());
    }
}
