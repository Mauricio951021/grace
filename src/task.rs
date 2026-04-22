use crate::sync::{ring::*, one_shot::*};
use crate::waker::*;
use crate::arena::*;


use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering::*, fence};
use std::pin::Pin;
use std::task::{RawWakerVTable, Waker, RawWaker};
use std::cell::UnsafeCell;
use std::mem;




pub(crate) struct Task<F: Future<Output = T> + 'static, T: Send + 'static>(pub(crate) *const TaskInner<F, T>);

unsafe impl<F: Future<Output = T> + Send + 'static, T: Send + 'static> Send for Task<F, T> {}
unsafe impl<F: Future<Output = T> + Sync + 'static, T: Send + 'static> Sync for Task<F, T> {}

impl<F, T> Clone for Task<F, T> 
where 
F: Future<Output = T> + 'static,
T: Send + 'static,
{
    fn clone(&self) -> Self {
        if self.data().ref_counter.fetch_add(1, Relaxed) >= usize::MAX / 2 {
            std::process::abort();
        }
        Task(self.data() as *const _)
    }
}

impl<F, T> Drop for Task<F, T> 
where 
F: Future<Output = T> + 'static,
T: Send + 'static,
{
    fn drop(&mut self) {
        if self.data().ref_counter.fetch_sub(1, Relaxed) == 1 {
            fence(Acquire);
            unsafe {
                let _ = Box::from_raw(self.0 as *mut TaskInner<F, T>);
            }
        }
    }
}

const V_TABLE: &RawWakerVTable = &RawWakerVTable::new(VTable::clone, VTable::wake, VTable::wake_by_ref, VTable::drop);

impl<F, T> Task<F, T> 
where 
F: Future<Output = T> + 'static,
T: Send + 'static,
{

    

    pub(crate) fn data(&self) -> &TaskInner<F, T> 
    where 
    F: Future<Output = T>,
    {
        unsafe { &*self.0 }
    }

    pub(crate) fn get_future(&self) -> Option<ArenaBox<F>> {
        unsafe { (*self.data().future.get()).take() }
    }

    pub(crate) fn waker(&self) -> Waker {
        let clon = self.clone();
        mem::forget(clon);
        let data = self.0 as *const ();
        let vtable = V_TABLE;
        let raw_waker = RawWaker::new(data, vtable);
        unsafe { Waker::from_raw(raw_waker) }
    }
}


pub(crate) struct TaskInner<F, T> 
where 
F: Future<Output = T> + 'static,
T: Send + 'static,
{
    pub(crate) ref_counter: AtomicUsize,
    pub(crate) id: u64,
    //primer bit = en ejecucion, segundo bit = notificacion para volver a ejecutar.
    pub(crate) state: AtomicU8,
    pub(crate) future: UnsafeCell<Option<ArenaBox<F>>>,
    pub(crate) sender: Sender<T>,
    pub(crate) ring: SyncRing,
    pub(crate) multi_thread: u8,
}

