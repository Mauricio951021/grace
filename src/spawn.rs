use crate::global::*;
use crate::task::*;
use crate::sync::one_shot::*;
use crate::arena::*;


use std::any::Any;
use std::mem::{self, ManuallyDrop};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::{Context, Poll};
use std::sync::atomic::{AtomicUsize, AtomicU8, Ordering::*};
use std::cell::UnsafeCell;
use std::pin::Pin;
use std::u8;



pub(crate) fn spawn_root_task(fut: impl Future<Output = ()> + 'static) {
    let world = unsafe {
        WORLD.assume_init_ref()
    };
    let number = world.global_counter_for_alternative_wake.load(Relaxed);
    let arena_box = world.arena.arena_alloc(number as usize, fut);
    let future = unsafe {
        UnsafeCell::new(Some(Pin::new_unchecked(ArenaBox::from_raw(arena_box.data as *mut dyn Future<Output = ()>,
        arena_box.metadata()))
    ))};
    mem::forget(arena_box);
    let mut task_inner = world.arena.arena_alloc(number as usize, TaskInner {
        ref_counter: AtomicUsize::new(1),
        id: world.task_id.fetch_add(1, Relaxed),
        state: AtomicU8::new(1),
        future,
        ring: world.single_thread_ring,
        multi_thread: u8::MAX,
        task_ptr_metadata: unsafe {mem::zeroed()},
    });
    task_inner.task_ptr_metadata = task_inner.metadata();
    let task = Task {
        data: ManuallyDrop::new(task_inner)
    };
    world.task_counter.fetch_add(1, Relaxed);
    world.single_thread_ring.inner().push_back(task);
}

/* 
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

pub struct JoinHandle<T>(Receiver<Result<T, Box<dyn Any + Send + 'static>>>);

impl<T> Future for JoinHandle<T> 
where 
T: Send + 'static,
{
type Output = Result<T, Box<dyn Any + Send + 'static>>;
fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let project = unsafe {
        Pin::new_unchecked(&mut self.get_unchecked_mut().0)
    };
    match project.poll(cx) {
        Poll::Ready(result) => {
            match result {
                Ok(ok) => Poll::Ready(ok),
                Err(_) => Poll::Ready(Err(Box::new(()))),
            }
        }
        Poll::Pending => Poll::Pending,
    }
}
}

pub fn spawn<F, T>(fut: F) -> JoinHandle<T> 
where 
F: Future<Output = T> + Send + 'static,
T: Send + 'static,
{
    let world = unsafe {
        WORLD.assume_init_ref()
    };
    let number = world.global_counter_for_alternative_wake.load(Relaxed);
    let (sender, receiver) = OneShot::new();
    let fut = TaskFuture {
        sender,
        fut,
    };
    let arena_box = world.arena.arena_alloc(number as usize, fut);
    let ptr = arena_box.data as *mut dyn Future<Output = ()>;
    let metadata = arena_box.metadata();
    let future = unsafe {
            UnsafeCell::new(Some(
            Pin::new_unchecked(ArenaBox::from_raw(ptr, metadata))
    ))};
    mem::forget(arena_box);
    
    let mut task = Task {
        data: ManuallyDrop::new(world.arena.arena_alloc(number as usize, TaskInner {
            ref_counter: AtomicUsize::new(1),
            id: world.task_id.fetch_add(1, Relaxed),
            state: AtomicU8::new(1),
            future,
            ring: world.multi_thread_ring,
            multi_thread: 1,
            task_ptr_metadata: unsafe {mem::zeroed()},
        })),
    };
    task.data.task_ptr_metadata = task.data.metadata();
    world.task_counter.fetch_add(1, Relaxed);
    world.multi_thread_ring.inner().push_back(task);
    let map = world.workers_bitmap.load(Relaxed);
    if map == 0 {
        return JoinHandle(receiver);
    }
    let idx = if (number & 1) == 0 {
        map.trailing_zeros() as usize
    } else {
        (63 - map.leading_zeros()) as usize
    };
    world.workers_bitmap.fetch_and(!(1 << idx), Relaxed);
    world.workers_id[idx].unpark();
    JoinHandle(receiver)
}

struct TaskFuture<F, T> {
    sender: Sender<Result<T, Box<dyn Any + Send + 'static>>>,
    fut: F,
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