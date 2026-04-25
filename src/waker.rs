use crate::arena::ArenaBox;
use crate::task::*;
use crate::global::*;

use std::mem::ManuallyDrop;
use std::sync::atomic::{Ordering::*};
use std::task::{RawWaker, RawWakerVTable};
use std::mem;


pub(crate) struct VTable;

impl VTable {


    pub(crate) fn clone(data: *const ()) -> RawWaker {
        let ptr = data as *mut TaskInner;
        let metadata = unsafe {
            (*ptr).metadata
        };
        let task = Task {
            data: ManuallyDrop::new(ArenaBox::from_raw(ptr, metadata))
        };
        let clone = task.clone();
        mem::forget(task);
        mem::forget(clone);
        RawWaker::new(data,V_TABLE)
    }

    pub(crate) fn wake(data: *const ()) {
        let ptr = data as *mut TaskInner;
        let metadata = unsafe {
            (*ptr).metadata
        };
        let task = Task {
            data: ManuallyDrop::new(ArenaBox::from_raw(ptr, metadata))
        };
        let ring = task.data().ring;
        let mut state = task.data().state.load(Relaxed);
        loop {
            if (state & 1) == 1 {
                match task
                    .data()
                    .state
                    .compare_exchange(state, 3, Release, Relaxed)
                {
                    Ok(_) => return,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            } else {
                match task
                    .data()
                    .state
                    .compare_exchange(state, 1, Relaxed, Relaxed)
                {
                    Ok(_) => break,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            }
        }
        let is_multi = task.data().multi_thread;
        ring.inner().push_back(task);
        if is_multi == 1 {
            let order = GLOBAL_COUNTER_FOR_ALTERNATIVE_WAKE.fetch_add(1, Relaxed);
            let map = WORKERS_BITMAP.load(Relaxed);
            if map == 0 {
                return;
            }
            let idx = if (order & 1) == 0 {
                map.trailing_zeros() as usize
            } else {
                (63 - map.leading_zeros()) as usize
            };
            WORKERS_BITMAP.fetch_and(!(1 << idx), Relaxed);
            unsafe {
                WORKERS_ID.get().unwrap()[idx].assume_init_ref().unpark();
            }
        } else {
            CURRENT_ID.get().unwrap().unpark();
        }
    }

    pub(crate) fn wake_by_ref(data: *const ()) {
        let ptr = data as *mut TaskInner;
        let metadata = unsafe {
            (*ptr).metadata
        };
        let task = Task {
            data: ManuallyDrop::new(ArenaBox::from_raw(ptr, metadata))
        };
        let ring = task.data().ring;
        let mut state = task.data().state.load(Relaxed);
        loop {
            if (state & 1) == 1 {
                match task
                    .data()
                    .state
                    .compare_exchange(state, 3, Release, Relaxed)
                {
                    Ok(_) => {
                        mem::forget(task);
                        return;
                    }
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            } else {
                match task
                    .data()
                    .state
                    .compare_exchange(state, 1, Relaxed, Relaxed)
                {
                    Ok(_) => break,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            }
        }
        let is_multi = task.data().multi_thread;
        ring.inner().push_back(task.clone());
        mem::forget(task);
        if is_multi == 1 {
            let order = GLOBAL_COUNTER_FOR_ALTERNATIVE_WAKE.fetch_add(1, Relaxed);
            let map = WORKERS_BITMAP.load(Relaxed);
            if map == 0 {
                return;
            }
            let idx = if (order & 1) == 0 {
                map.trailing_zeros() as usize
            } else {
                (63 - map.leading_zeros()) as usize
            };
            WORKERS_BITMAP.fetch_and(!(1 << idx), Relaxed);
            unsafe {
                WORKERS_ID.get().unwrap()[idx].assume_init_ref().unpark();
            }
        } else {
            CURRENT_ID.get().unwrap().unpark();
        }
    }

    pub (crate) fn drop(data: *const ()) {
        let ptr = data as *mut TaskInner;
        let metadata = unsafe {
            (*ptr).metadata
        };
        let _ = Task {
            data: ManuallyDrop::new(ArenaBox::from_raw(ptr, metadata))
        };
    }
}