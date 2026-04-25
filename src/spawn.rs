use crate::global::*;
use crate::task::*;
use crate::sync::one_shot::*;


use std::any::Any;
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::{Context, Poll};
use std::sync::atomic::{AtomicUsize, AtomicU8, Ordering::*};
use std::cell::UnsafeCell;
use std::thread;
use std::pin::Pin;


/* 
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
*/
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



struct JoinHandle<T>(Receiver<Result<T, Box<dyn Any + Send + 'static>>>);

fn spawn<F, T>(fut: F) -> JoinHandle<T> 
where 
F: Future<Output = T> + Send + 'static,
T: Send + 'static,
{
    let world = unsafe {
        WORLD.assume_init_ref()
    };
    let number = world.global_counter_for_alternative_wake.load(Relaxed);
    let (sender, receiver) = OneShot::new();
    let future = UnsafeCell::new(Some(world.arena.arena_alloc(number as usize, TaskFuture {
        fut,
        sender,
    })));
    JoinHandle(receiver)
}

struct TaskFuture<F, T> {
    fut: F,
    sender: Sender<Result<T, Box<dyn Any + Send + 'static>>>,
}



impl<F, T> Future for TaskFuture<F, T>
where 
F: Future<Output = T> + Send + 'static,
T: Send + 'static,
{
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let project = unsafe {
            Pin::new_unchecked(&mut self.as_mut().get_unchecked_mut().fut)
        };
        let assert_unwind = AssertUnwindSafe(move || project.poll(cx));
        match catch_unwind(assert_unwind) {
            Ok(ok) => {
                match ok {
                    Poll::Ready(result) => {
                        self.sender.internal_send(Ok(result));
                        Poll::Ready(())
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(e) => {
                self.sender.internal_send(Err(e));
                Poll::Ready(())
            }
        }
    }
}