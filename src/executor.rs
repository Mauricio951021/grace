use crate::sync::ring::*;
use crate::global::*;
use crate::spawn::*;

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering::*, fence};
use std::thread::{self, Thread};
use std::mem::MaybeUninit;
use std::task::Context;


pub struct Executor {
    marker: PhantomData<*mut ()>,
}


impl Executor {
    pub fn new() -> Self {
        static FIRST_TIME: AtomicBool = AtomicBool::new(true);
        assert!(FIRST_TIME.swap(false, Relaxed));
        let rt = Self {
            marker: PhantomData,
        };
        let _ = CURRENT_ID.set(thread::current());
        let _ = CURRENT.set(SyncRing::new());
        let _ = MULTI.set(SyncRing::new());
        let mut cores = num_cpus::get();
        assert!(cores > 0);
        if cores > 64 {
            cores = 64
        }
        let mut threads_ids = Vec::with_capacity(cores);
        for _ in 0..cores {
            threads_ids.push(MaybeUninit::uninit());
        }
        let _ = WORKERS_ID.set(threads_ids);

        let map = if cores == 64 {
            u64::MAX
        } else {
            (1 << cores) - 1
        };
        WORKERS_BITMAP.fetch_or(map, Relaxed);

        for i in 0..cores {
            std::thread::spawn(move || {
                let worker_idx = i;
                let worker_flag = (1 << worker_idx) as u64;
                unsafe {
                    (*(WORKERS_ID.get().unwrap().as_ptr() as *mut MaybeUninit<Thread>)
                        .add(worker_idx))
                    .write(thread::current());
                }
                READY.fetch_add(1, Release);
                let mut spin_loop_counter: u8 = 0;
                thread::park();
                loop {
                    while let Some(t) = MULTI.get().unwrap().inner().multithread_consume() {
                        spin_loop_counter = 0;
                        let waker = t.waker();
                        let mut cx = Context::from_waker(&waker);
                        let mut fut = t.get_future();
                        'tag: loop {
                            if let Some(mut f) = fut {
                                let mut state;
                                loop {
                                    if f.as_mut().poll(&mut cx).is_pending() {
                                        state = t.data().state.load(Relaxed);
                                        if (2 & state) == 2 {
                                            t.data().state.fetch_and(1, Acquire);
                                            continue;
                                        } else {
                                            unsafe {
                                                (*t.data().future.get()) = Some(f);
                                            }
                                            match t
                                                .data()
                                                .state
                                                .compare_exchange(state, 0, Relaxed, Relaxed)
                                            {
                                                Ok(_) => break 'tag,
                                                Err(_) => {
                                                    fut = t.get_future();
                                                    t.data().state.fetch_and(1, Acquire);
                                                    continue 'tag;
                                                }
                                            }
                                        }
                                    } else {
                                        TASK_COUNTER.fetch_sub(1, Relaxed);
                                        break 'tag;
                                    }
                                }
                            }
                            break;
                        }
                    }
                    if spin_loop_counter < 5 {
                        std::hint::spin_loop();
                        if spin_loop_counter == 4 {
                            WORKERS_BITMAP.fetch_or(worker_flag, Relaxed);
                        }
                        spin_loop_counter += 1;
                        continue;
                    }
                    spin_loop_counter = 0;
                    thread::park();
                }
            });
        }
        while READY.load(Relaxed) < cores as u64 {
            std::hint::spin_loop();
        }
        fence(Acquire);
        rt
    }

    pub fn block_on(&self, fut: impl Future<Output = ()> + 'static) {
        assert!(thread::current().id() == CURRENT_ID.get().unwrap().id());
        spawn_root_task(fut);
        let mut spin_loop_counter = 0u8;
        loop {
            while let Some(t) = CURRENT.get().unwrap().inner().consume() {
                spin_loop_counter = 0;
                let waker = t.waker();
                let mut cx = Context::from_waker(&waker);
                if let Some(mut f) = t.get_future() {
                    if f.as_mut().poll(&mut cx).is_pending() {
                        unsafe {
                            (*t.data().future.get()) = Some(f);
                        }
                    } else {
                        if t.data().multi_thread == u8::MAX {
                            return;
                        }
                    }
                }
            }
            if spin_loop_counter < 3 {
                spin_loop_counter += 1;
                continue;
            }
            thread::park();
        }
    }
}

//fn unwind_safe_poll(task: Task)