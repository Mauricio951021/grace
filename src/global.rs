use crate::sync::ring::*;
use crate::arena::*;

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::thread::Thread;
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};


pub(crate) static WORLD: MaybeUninit<World> = MaybeUninit::uninit();
pub(crate) static WORLD_READY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub(crate) struct World {
    pub(crate) data: *mut WorldData,
}

unsafe impl Send for World {}
unsafe impl Sync for World {}

impl Deref for World {
    type Target = WorldData;
    fn deref(&self) -> &Self::Target {
        unsafe {
            &*self.data
        }
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            &mut*self.data
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
        };
        Self { data: Box::into_raw(Box::new(data)) }
    } 
}

#[repr(align(64))]
pub(crate) struct Aligned<T>(T);

impl<T> Deref for Aligned<T>  {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}