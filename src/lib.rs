use std::cell::UnsafeCell;
use std::pin::Pin;
use std::thread::{self, Thread};
use std::{
    alloc::{Layout, alloc},
    marker::PhantomData,
    mem::{self, MaybeUninit},
    sync::{
        OnceLock,
        atomic::{Ordering::*, *},
    },
    task::{Context, RawWaker, RawWakerVTable, Waker},
    u8, u64,
};

static CURRENT: OnceLock<SyncRing> = OnceLock::new();
static CURRENT_ID: OnceLock<Thread> = OnceLock::new();
static MULTI: OnceLock<SyncRing> = OnceLock::new();
static WORKERS_BITMAP: AtomicU64 = AtomicU64::new(0);
static WORKERS_ID: OnceLock<Vec<MaybeUninit<Thread>>> = OnceLock::new();
static READY: AtomicU64 = AtomicU64::new(0);
static GLOBAL_COUNTER_FOR_ALTERNATIVE_WAKE: AtomicU64 = AtomicU64::new(1);
static TASK_ID: AtomicU64 = AtomicU64::new(0);
static TASK_COUNTER: AtomicU64 = AtomicU64::new(0);

fn spawn_root_task(fut: impl Future<Output = ()> + 'static) {
    let task = Tarea(Box::into_raw(Box::new(Inner {
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
    let task = Tarea(Box::into_raw(Box::new(Inner {
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

pub fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    let task = Tarea(Box::into_raw(Box::new(Inner {
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
                                    }
                                }
                            }
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
            if spin_loop_counter < 3 {}
        }
    }
}

struct Tarea(*const Inner);

impl Clone for Tarea {
    fn clone(&self) -> Self {
        if self.data().ref_counter.fetch_add(1, Relaxed) >= usize::MAX / 2 {
            std::process::abort();
        }
        Tarea(self.data() as *const _)
    }
}

impl Drop for Tarea {
    fn drop(&mut self) {
        if self.data().ref_counter.fetch_sub(1, Relaxed) == 1 {
            fence(Acquire);
            unsafe {
                let _ = Box::from_raw(self.0 as *mut Inner);
            }
        }
    }
}

impl Tarea {
    fn data(&self) -> &Inner {
        unsafe { &*self.0 }
    }

    fn get_future(&self) -> Option<Pin<Box<dyn Future<Output = ()>>>> {
        unsafe { (*self.data().future.get()).take() }
    }
}

struct Inner {
    ref_counter: AtomicUsize,
    #[allow(dead_code)]
    id: u64,
    //primer bit = en ejecucion, segundo bit = notificacion para volver a ejecutar.
    state: AtomicU8,
    future: UnsafeCell<Option<Pin<Box<dyn Future<Output = ()> + 'static>>>>,
    ring: SyncRing,
    multi_thread: u8,
}

unsafe impl Send for Tarea {}
unsafe impl Sync for Tarea {}

impl Tarea {
    fn waker(&self) -> Waker {
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

#[derive(Clone, Copy)]
struct SyncRing {
    ptr: *mut RawSyncRing,
}

unsafe impl Send for SyncRing {}
unsafe impl Sync for SyncRing {}

impl SyncRing {
    fn inner(&self) -> &RawSyncRing {
        unsafe { &*self.ptr }
    }
    fn new() -> Self {
        let ptr = Box::into_raw(Box::new(RawSyncRing::new(4096)));
        Self { ptr }
    }
}

struct RawSyncRing {
    ring_mask: usize,
    ptr: *mut Tarea,
    head: AtomicUsize,
    multi_thread_head: AtomicUsize,
    writers_tail: AtomicUsize,
    rt_tail: AtomicUsize,
}
unsafe impl Send for RawSyncRing {}
unsafe impl Sync for RawSyncRing {}

impl RawSyncRing {
    fn new(cap: usize) -> Self {
        assert!(cap <= isize::MAX as usize);
        assert!(cap.is_power_of_two());
        let layout = Layout::array::<Tarea>(cap).unwrap();
        let ptr = unsafe { alloc(layout) as *mut Tarea };
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
    fn push_back(&self, t: Tarea) {
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
    fn consume(&self) -> Option<Tarea> {
        let head = self.head.load(Relaxed);
        if head == self.rt_tail.load(Acquire) {
            return None;
        }
        let res = unsafe { Some(self.ptr.add(head & self.ring_mask).read()) };
        self.head.store(head.wrapping_add(1), Release);
        res
    }
    fn multithread_consume(&self) -> Option<Tarea> {
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

struct VTable;

impl VTable {
    fn clone(data: *const ()) -> RawWaker {
        let task = Tarea(data as *const _);
        let clone = task.clone();
        mem::forget(task);
        mem::forget(clone);
        RawWaker::new(
            data,
            &RawWakerVTable::new(
                VTable::clone,
                VTable::wake,
                VTable::wake_by_ref,
                VTable::drop,
            ),
        )
    }

    fn wake(data: *const ()) {
        let task = Tarea(data as *const Inner);
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

    fn wake_by_ref(data: *const ()) {
        let task = Tarea(data as *const Inner);
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

    fn drop(data: *const ()) {
        let _ = Tarea(data as *const _);
    }
}
