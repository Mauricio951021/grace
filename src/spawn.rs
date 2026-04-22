use crate::global::*;
use crate::task::*;
use crate::sync::one_shot::*;


use std::sync::atomic::{AtomicUsize, AtomicU8, Ordering::*};
use std::cell::UnsafeCell;
use std::thread;

pub(crate) fn spawn_root_task(fut: impl Future<Output = ()> + 'static) {
    let task = Task(Box::into_raw(Box::new(Inner {
        ref_counter: AtomicUsize::new(1),
        id: TASK_ID.fetch_add(1, Relaxed),
        state: AtomicU8::new(1),
        future: UnsafeCell::new(Some(Box::pin(fut))),
        ring: *CURRENT.get().unwrap(),
        multi_thread: u8::MAX,
    })));
    TASK_COUNTER.fetch_add(1, Relaxed);
    CURRENT.get().unwrap().inner().push_back(task);
    CURRENT_ID.get().unwrap().unpark();
}

pub fn spawn_local(fut: impl Future<Output = ()> + 'static) {
    assert!(thread::current().id() == CURRENT_ID.get().unwrap().id());
    let task = Task(Box::into_raw(Box::new(Inner {
        ref_counter: AtomicUsize::new(1),
        id: TASK_ID.fetch_add(1, Relaxed),
        state: AtomicU8::new(1),
        future: UnsafeCell::new(Some(Box::pin(fut))),
        ring: *CURRENT.get().unwrap(),
        multi_thread: 0,
    })));
    TASK_COUNTER.fetch_add(1, Relaxed);
    CURRENT.get().unwrap().inner().push_back(task);
    CURRENT_ID.get().unwrap().unpark();
}

pub fn spawn_multi(fut: impl Future<Output = ()> + Send + 'static) {
    let task = Task(Box::into_raw(Box::new(Inner {
        ref_counter: AtomicUsize::new(1),
        id: TASK_ID.fetch_add(1, Relaxed),
        state: AtomicU8::new(1),
        future: UnsafeCell::new(Some(Box::pin(fut))),
        ring: *MULTI.get().unwrap(),
        multi_thread: 1,
    })));
    MULTI.get().unwrap().inner().push_back(task);
    TASK_COUNTER.fetch_add(1, Relaxed);
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
}

/*struct JoinHandle<T>(Receiver<T>);

fn spawn<T, F>(fut: F) -> JoinHandle<T> 
where 
F: Future<Output = T> + Send + 'static,
T: Send + 'static,
{
    let (sender, receiver) = OneShot::<T>::new();
    let ptr = Box::new(sender);
    let task = Task(Box::into_raw(Box::new(Inner {
        ref_counter: AtomicUsize::new(1),
        id: TASK_ID.fetch_add(1, Relaxed),
        state: AtomicU8::new(1),
        future: UnsafeCell::new(Some(Box::pin(fut))),
        
    })));
}*/