use crate::ring::*;
use crate::waker::*;

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering::*, fence};
use std::pin::Pin;
use std::task::{RawWakerVTable, Waker, RawWaker};
use std::cell::UnsafeCell;
use std::mem;




pub(crate) struct Task(pub(crate) *const Inner);

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Clone for Task {
    fn clone(&self) -> Self {
        if self.data().ref_counter.fetch_add(1, Relaxed) >= usize::MAX / 2 {
            std::process::abort();
        }
        Task(self.data() as *const _)
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.data().ref_counter.fetch_sub(1, Relaxed) == 1 {
            fence(Acquire);
            unsafe {
                let _ = Box::from_raw(self.0 as *mut Inner);
            }
        }
    }
}

impl Task {
    pub(crate) fn data(&self) -> &Inner {
        unsafe { &*self.0 }
    }

    pub(crate) fn get_future(&self) -> Option<Pin<Box<dyn Future<Output = ()>>>> {
        unsafe { (*self.data().future.get()).take() }
    }

    pub(crate) fn waker(&self) -> Waker {
        let clon = self.clone();
        mem::forget(clon);
        let data = self.0 as *const ();
        let vtable = &RawWakerVTable::new(
            VTable::clone,
            VTable::wake,
            VTable::wake_by_ref,
            VTable::drop,
        );
        let raw_waker = RawWaker::new(data, vtable);
        unsafe { Waker::from_raw(raw_waker) }
    }
}


pub(crate) struct Inner {
    pub(crate) ref_counter: AtomicUsize,
    #[allow(dead_code)]
    pub(crate) id: u64,
    //primer bit = en ejecucion, segundo bit = notificacion para volver a ejecutar.
    pub(crate) state: AtomicU8,
    pub(crate) future: UnsafeCell<Option<Pin<Box<dyn Future<Output = ()> + 'static>>>>,
    pub(crate) ring: SyncRing,
    pub(crate) multi_thread: u8,
}