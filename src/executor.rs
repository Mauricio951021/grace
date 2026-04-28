use crate::global::*;
use crate::spawn::*;
use crate::config::*;

use std::marker::PhantomData;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{Ordering::*, fence};
use std::sync::Arc;
use std::thread::{self, Thread, Builder};
use std::task::Context;
use std::u64;


pub struct RT {
    marker: PhantomData<*mut ()>,
}


impl RT {
    pub(crate) fn new(mut config: GraceConfig) -> Self {
        let mut world = World::new(
        config.local_task_buffer_size,
            thread::current(),
            config.worker_task_buffer_size,
            config.arena_slots);
        assert!(config.worker_count != 0, "No pueden haber 0 workers");
        if config.worker_count >= 64 {
            config.worker_count = 64;
            world.workers_bitmap.store(u64::MAX, Relaxed);
        } else {
            world.workers_bitmap.store((1 << config.worker_count) - 1, Relaxed);
        }
        let mut workers_id: Vec<Thread> = Vec::with_capacity(config.worker_count);
        for _ in 0..config.worker_count {
            workers_id.push(thread::current());
        }
        let workers_id = Arc::new(workers_id);
        let ready = Arc::new(AtomicU8::new(0));
        
        for i in 0..config.worker_count {
            let workers_id_clon = workers_id.clone();
            let ready_clon = ready.clone();
            let builder = Builder::new().stack_size(config.worker_stack_size).name(format!("worker-{}", i + 1));
            let _ = builder.spawn(move || {
                let worker_idx = i;
                let worker_flag = (1 << worker_idx) as u64;
                unsafe {
                    ((*workers_id_clon).as_ptr() as *mut Thread).add(worker_idx).write(thread::current());
                    drop(workers_id_clon);
                }
                ready_clon.fetch_add(1, Release);
                drop(ready_clon);
                while !WORLD_READY.load(Acquire) {
                    std::hint::spin_loop();
                }
                let world = unsafe {
                    WORLD.assume_init_ref()
                };
                let mut spin_loop_count = 0u8;
                thread::park();
                loop {
                    while let Some(t) = world.multi_thread_ring.inner().multithread_consume() {
                        spin_loop_count = 0;
                        let waker = t.waker();
                        let mut cx = Context::from_waker(&waker);
                        let mut fut = t.get_future();
                        'tag: loop {
                            if let Some(mut f) = fut {
                                let mut state: u8;
                                loop {
                                    if f.as_mut().poll(&mut cx).is_pending() {
                                        state = t.data().state.load(Relaxed);
                                        if (2 & state) == 2 {
                                            t.data().state.fetch_and(1, Acquire);
                                            continue;
                                        } else {
                                            t.put_future(f);
                                            match t.data().state.compare_exchange(state, 0, Release, Relaxed) {
                                                Ok(_) => break 'tag,
                                                Err(_) => {
                                                    fut = t.get_future();
                                                    t.data().state.fetch_and(1, Acquire);
                                                    continue 'tag;
                                                }
                                            }
                                        }
                                    } else {
                                        world.task_counter.fetch_sub(1, Relaxed);
                                        break 'tag;
                                    }
                                }
                            }
                            break;
                        }
                    }
                    if spin_loop_count < 5 {
                        std::hint::spin_loop();
                        if spin_loop_count == 4 {
                            world.workers_bitmap.fetch_or(worker_flag, Relaxed);
                        }
                        spin_loop_count += 1;
                        continue;
                    }
                    spin_loop_count = 0;
                    thread::park();
                }
            });
        }
        
        while ready.load(Relaxed) != config.worker_count as u8 {
            std::hint::spin_loop();
        }
        fence(Acquire);
        world.workers_id = (*workers_id).clone();
        unsafe {
            (WORLD.as_ptr() as *mut World).write(world);
        }
        WORLD_READY.store(true, Release);

        RT { marker: PhantomData }
    }

    pub fn block_on(self, fut: impl Future<Output = ()> + 'static) {
        let world = unsafe {
            WORLD.assume_init_ref()
        };
        assert_eq!(thread::current().id(), world.main_id.id());
        spawn_root_task(fut);
        let mut spin_loop_count = 0u8;
        loop {
            while let Some(t) = world.single_thread_ring.inner().consume() {
                spin_loop_count = 0;
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
            if spin_loop_count < 3 {
                spin_loop_count += 1;
                continue;
            }
            thread::park();
        }

    }
}

