use crate::task::*;

use std::sync::atomic::{AtomicUsize, Ordering::*, fence};
use std::alloc::{alloc, Layout};


const DEFAULT_RING_CAP: usize = 4096;

#[derive(Clone, Copy)]
pub(crate) struct SyncRing {
    pub(crate) ptr: *mut RawSyncRing,
}

unsafe impl Send for SyncRing {}
unsafe impl Sync for SyncRing {}

impl SyncRing {
    pub(crate) fn inner(&self) -> &RawSyncRing {
        unsafe { &*self.ptr }
    }
    pub(crate) fn new(mut s: usize) -> Self {
        if s == 0 {
            s = DEFAULT_RING_CAP
        }
        let ptr = Box::into_raw(Box::new(RawSyncRing::new(s)));
        Self { ptr }
    }
}

pub(crate) struct RawSyncRing {
    pub(crate) ring_mask: usize,
    pub(crate) ptr: *mut Task,
    pub(crate) head: AtomicUsize,
    pub(crate) multi_thread_head: AtomicUsize,
    pub(crate) writers_tail: AtomicUsize,
    pub(crate) rt_tail: AtomicUsize,
}
unsafe impl Send for RawSyncRing {}
unsafe impl Sync for RawSyncRing {}

impl RawSyncRing {

    pub(crate) fn new(cap: usize) -> Self {
        assert!(cap <= isize::MAX as usize);
        assert!(cap.is_power_of_two());
        let layout = Layout::array::<Task>(cap).unwrap();
        let ptr = unsafe { alloc(layout) as *mut Task };
        assert!(!ptr.is_null());

        RawSyncRing {
            ring_mask: cap - 1,
            ptr,
            head: AtomicUsize::new(0),
            multi_thread_head: AtomicUsize::new(0),
            writers_tail: AtomicUsize::new(0),
            rt_tail: AtomicUsize::new(0),
        }
    }

    pub(crate) fn push_back(&self, t: Task) {
        let mut tail = self.writers_tail.load(Relaxed);
        loop {
            if tail.wrapping_sub(self.head.load(Acquire)) >= self.ring_mask + 1 {
                std::hint::spin_loop();
                tail = self.writers_tail.load(Relaxed);
                continue;
            }
            match self
                .writers_tail
                .compare_exchange(tail, tail.wrapping_add(1), Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(e) => {
                    tail = e;
                    continue;
                }
            }
        }
        unsafe {
            self.ptr.add(tail & self.ring_mask).write(t);
        }
        while tail != self.rt_tail.load(Relaxed) {
            std::hint::spin_loop();
        }
        self.rt_tail.store(tail.wrapping_add(1), Release);
    }

    pub(crate) fn consume(&self) -> Option<Task> {
        let head = self.head.load(Relaxed);
        if head == self.rt_tail.load(Acquire) {
            return None;
        }
        let res = unsafe { Some(self.ptr.add(head & self.ring_mask).read()) };
        self.head.store(head.wrapping_add(1), Release);
        res
    }

    pub(crate) fn multithread_consume(&self) -> Option<Task> {
        let mut multithread_head = self.multi_thread_head.load(Relaxed);
        loop {
            if multithread_head == self.rt_tail.load(Relaxed) {
                return None;
            }
            fence(Acquire);
            match self.multi_thread_head.compare_exchange(
                multithread_head,
                multithread_head.wrapping_add(1),
                Relaxed,
                Relaxed,
            ) {
                Ok(_) => break,
                Err(e) => {
                    multithread_head = e;
                    continue;
                }
            }
        }
        let res = unsafe { Some(self.ptr.add(multithread_head & self.ring_mask).read()) };
        while multithread_head != self.head.load(Relaxed) {
            std::hint::spin_loop();
        }
        self.head.store(multithread_head.wrapping_add(1), Release);
        res
    }
}