use crate::ring::*;

use std::sync::atomic::AtomicU64;
use std::sync::OnceLock;
use std::thread::Thread;
use std::mem::MaybeUninit;




pub(crate) static CURRENT: OnceLock<SyncRing> = OnceLock::new();
pub(crate) static CURRENT_ID: OnceLock<Thread> = OnceLock::new();
pub(crate) static MULTI: OnceLock<SyncRing> = OnceLock::new();
pub(crate) static WORKERS_BITMAP: AtomicU64 = AtomicU64::new(0);
pub(crate) static WORKERS_ID: OnceLock<Vec<MaybeUninit<Thread>>> = OnceLock::new();
pub(crate) static READY: AtomicU64 = AtomicU64::new(0);
pub(crate) static GLOBAL_COUNTER_FOR_ALTERNATIVE_WAKE: AtomicU64 = AtomicU64::new(1);
pub(crate) static TASK_ID: AtomicU64 = AtomicU64::new(0);
pub(crate) static TASK_COUNTER: AtomicU64 = AtomicU64::new(0);