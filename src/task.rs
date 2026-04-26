use crate::sync::ring::*;
use crate::waker::*;
use crate::arena::*;


use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering::*, fence};
use std::task::{RawWakerVTable, Waker, RawWaker};
use std::cell::UnsafeCell;
use std::mem::{self, ManuallyDrop};




pub(crate) struct Task {
    pub(crate) data: ManuallyDrop<ArenaBox<TaskInner>>,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Clone for Task {
    fn clone(&self) -> Self {
        if self.data().ref_counter.fetch_add(1, Relaxed) >= usize::MAX / 2 {
            std::process::abort();
        }
        unsafe {
            Task {
                data: ((&self.data) as *const ManuallyDrop<ArenaBox<TaskInner>>).read(),
            }
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.data().ref_counter.fetch_sub(1, Relaxed) == 1 {
            fence(Acquire);
            unsafe {
                ManuallyDrop::drop(&mut self.data);
            }
        }
    }
}

pub(crate) const V_TABLE: &RawWakerVTable = &RawWakerVTable::new(VTable::clone, VTable::wake, VTable::wake_by_ref, VTable::drop);

impl Task {
    pub(crate) fn data(&self) -> &TaskInner {
        &self.data
    }

    pub(crate) fn get_future(&self) -> Option<impl Future> {
        unsafe {
            if let Some(f) = (*self.data().future.get()).take() {
                Some(f)
            } else {
                None
            }
        }
    }

    pub(crate) fn put_future(&self, f: Pin<ArenaBox<dyn Future<Output = ()> + 'static>>) {
        unsafe {
            (*self.data().future.get()) = Some(f);
        }
    }

    pub(crate) fn waker(&self) -> Waker {
        let clon = self.clone();
        mem::forget(clon);
        let data = self.data.data as *const ();
        let vtable = V_TABLE;
        let raw_waker = RawWaker::new(data, vtable);
        unsafe { Waker::from_raw(raw_waker) }
    }
}


pub(crate) struct TaskInner {
    pub(crate) ref_counter: AtomicUsize,
    pub(crate) id: u64,
    //primer bit = en ejecucion, segundo bit = notificacion para volver a ejecutar.
    pub(crate) state: AtomicU8,
    pub(crate) future: UnsafeCell<Option<Pin<ArenaBox<dyn Future<Output = ()>>>>>,
    pub(crate) ring: SyncRing,
    pub(crate) multi_thread: u8,
    pub(crate) task_ptr_metadata: Metadata,
}

