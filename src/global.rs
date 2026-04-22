use crate::sync::ring::*;
use crate::arena::*;

use std::sync::atomic::{AtomicU64, AtomicU8};
use std::sync::OnceLock;
use std::thread::Thread;
use std::mem::MaybeUninit;
use std::ops::Deref;




pub(crate) static CURRENT: OnceLock<SyncRing> = OnceLock::new();
pub(crate) static CURRENT_ID: OnceLock<Thread> = OnceLock::new();
pub(crate) static MULTI: OnceLock<SyncRing> = OnceLock::new();
pub(crate) static WORKERS_BITMAP: AtomicU64 = AtomicU64::new(0);
pub(crate) static WORKERS_ID: OnceLock<Vec<MaybeUninit<Thread>>> = OnceLock::new();
pub(crate) static READY: AtomicU64 = AtomicU64::new(0);
pub(crate) static GLOBAL_COUNTER_FOR_ALTERNATIVE_WAKE: AtomicU64 = AtomicU64::new(1);
pub(crate) static TASK_ID: AtomicU64 = AtomicU64::new(0);
pub(crate) static TASK_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub(crate) struct World {
    pub(crate) data: *mut WorldData,
}

impl Deref for World {
    type Target = WorldData;
    fn deref(&self) -> &Self::Target {
        unsafe {
            &*self.data
        }
    }
}

pub(crate) struct WorldData {
    pub(crate) single_thread_ring: SyncRing,
    pub(crate) main_id: Thread,
    pub(crate) multi_thread_ring: SyncRing,
    pub(crate) workers_bitmap: Aligned<AtomicU64>,
    pub(crate) workers_id: Vec<Thread>,
    pub(crate) global_counter_for_alternative_wake: Aligned<AtomicU64>,
    pub(crate) task_id: Aligned<AtomicU64>,
    pub(crate) task_counter: Aligned<AtomicU64>,
    pub(crate) arena: Arena,
    pub(crate) world_ready: AtomicU8,
    reserv: [u8; 128],
}

impl World {
    pub(crate) fn new(single_ring_s: usize, main_id: Thread, multi_ring_s: usize, arena_s: usize) -> Self {
        let data = WorldData {
            single_thread_ring: SyncRing::new(single_ring_s),
            main_id,
            multi_thread_ring: SyncRing::new(multi_ring_s),
            workers_bitmap: Aligned(AtomicU64::new(0)),
            workers_id: Vec::new(),
            global_counter_for_alternative_wake: Aligned(AtomicU64::new(1)),
            task_id: Aligned(AtomicU64::new(0)),
            task_counter: Aligned(AtomicU64::new(0)),
            arena: Arena::new(arena_s),
            world_ready: AtomicU8::new(0),
            reserv: [0; 128],
        };
        Self { data: Box::into_raw(Box::new(data)) }
    } 
}

#[repr(align(64))]
struct Aligned<T>(T);

impl<T> Deref for Aligned<T>  {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}