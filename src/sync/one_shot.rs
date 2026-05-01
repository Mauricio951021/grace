use crate::arena::*;
use crate::global::WORLD;


use std::cell::UnsafeCell;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::sync::atomic::{AtomicU8, Ordering::*, fence};


// the sender is alive
const SENDER: u8 = 1;

//the receiver is alive
const RECEIVER: u8 = 2;

const MESSAGE_READY: u8 = 4;

const WAKER_READY: u8 = 8;

const TERMINATED: u8 = 16;

pub(crate) struct InternalSender<T> {
    channel: UnsafeCell<ManuallyDrop<ArenaBox<InternalOneshot<T>>>>,
}
unsafe impl<T: Send> Send for InternalSender<T> {}
unsafe impl<T: Sync> Sync for InternalSender<T> {}

pub(crate) struct InternalReceiver<T> {
    channel: UnsafeCell<ManuallyDrop<ArenaBox<InternalOneshot<T>>>>,
}
unsafe impl<T: Send> Send for InternalReceiver<T> {}
unsafe impl<T: Sync> Sync for InternalReceiver<T> {}

pub(crate) struct InternalOneshot<T> {
    state: AtomicU8,
    message: UnsafeCell<MaybeUninit<T>>,
    waker: UnsafeCell<MaybeUninit<Waker>>,
}

unsafe impl<T: Send> Send for InternalOneshot<T> {}
unsafe impl<T: Sync> Sync for InternalOneshot<T> {}

impl<T> InternalOneshot<T> {
    pub(crate) fn new(rand_idx: usize) -> (InternalSender<T>, InternalReceiver<T>) {
        let arena = unsafe {
            &WORLD.assume_init_ref().arena
        };
        let channel = InternalOneshot {
            state: AtomicU8::new(SENDER | RECEIVER),
            message: UnsafeCell::new(MaybeUninit::uninit()),
            waker: UnsafeCell::new(MaybeUninit::uninit()),
        };
        let ptr1 = arena.arena_alloc(rand_idx, channel);
        let ptr2 = UnsafeCell::new(ManuallyDrop::new(ArenaBox::from_raw(ptr1.data, ptr1.metadata())));
        (InternalSender {channel: UnsafeCell::new(ManuallyDrop::new(ptr1))}, InternalReceiver {channel: ptr2})
    }
}

impl<T> InternalSender<T> {
    pub(crate) fn send(&self, val: T) {
        let channel = unsafe {
            &**self.channel.get()
        };
        let mut state = channel.state.load(Relaxed);
        if (state & RECEIVER) == RECEIVER {
            unsafe {
                (&mut*channel.message.get()).write(val);
            }
            state = channel.state.fetch_or(MESSAGE_READY, Release) | MESSAGE_READY;
            loop {
                if (state & WAKER_READY) == WAKER_READY {
                    match channel.state.compare_exchange(state, state & (!WAKER_READY), Acquire, Relaxed) {
                        Ok(_) => {
                            unsafe {
                                (&mut*channel.waker.get()).assume_init_read().wake();
                            }
                            return;
                        }
                        Err(e) => {
                            state = e;
                            continue;
                        }
                    }
                }
                return;
            }
        }
    }
}

impl<T> Drop for InternalSender<T> {
    fn drop(&mut self) {
        let channel = unsafe {
            &**self.channel.get()
        };
        let mut state = channel.state.load(Relaxed);
        loop {
            if (state & RECEIVER) == RECEIVER {
                match channel.state.compare_exchange(state, state & (!SENDER), Relaxed, Relaxed) {
                    Ok(_) => return,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            }
            fence(Acquire);
            if (state & MESSAGE_READY) == MESSAGE_READY {
                unsafe {
                    (&mut*channel.message.get()).assume_init_drop();
                }
            }
            if (state & WAKER_READY) == WAKER_READY {
                unsafe {
                    (&mut*channel.waker.get()).assume_init_drop();
                }
            }
            unsafe {
                ManuallyDrop::drop(&mut*self.channel.get());
            }
            return;
        }
    }
}

impl<T> Future for InternalReceiver<T> {
    type Output = Result<T, ()>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let channel = unsafe {
            &**self.channel.get()
        };
        let mut state = channel.state.load(Relaxed);
        if (state & TERMINATED) == TERMINATED {
            panic!("can not poll")
        }
        let mut waker_set = if (state & WAKER_READY) == WAKER_READY {
            let mut was_me = false;
            loop {
                match channel.state.compare_exchange(state, state & (!WAKER_READY), Relaxed, Relaxed) {
                    Ok(_) => {
                        was_me = true;
                        break;
                    }
                    Err(e) => {
                        state = e;
                        if (state & WAKER_READY) == WAKER_READY {
                            continue;
                        }
                        break;
                    } 
                }
            }
            if was_me {
                unsafe {
                    if (&*channel.waker.get()).assume_init_ref().will_wake(cx.waker()) {
                        channel.state.fetch_or(WAKER_READY, Release);
                        true
                    } else {
                        (&mut*channel.waker.get()).assume_init_drop();
                        false
                    }
                }
            } else {
                true
            }
        } else {
            false
        };
        if (state & MESSAGE_READY) == MESSAGE_READY {
            fence(Acquire);
            let result = unsafe {
                Ok((&*channel.message.get()).assume_init_read())
            };
            channel.state.fetch_and(!MESSAGE_READY, Relaxed);
            channel.state.fetch_or(TERMINATED, Relaxed);
            return Poll::Ready(result);
        }
        loop {
            if ((state & SENDER) == SENDER) && ((state & MESSAGE_READY) == 0) {
                if !waker_set {
                    let waker = cx.waker().clone();
                    unsafe {
                        (&mut*channel.waker.get()).write(waker);
                    }
                    waker_set = true;
                }
                match channel.state.compare_exchange(state, state | WAKER_READY, Release, Relaxed) {
                    Ok(_) => return Poll::Pending,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            } 
            if ((state & SENDER) == 0) && ((state & MESSAGE_READY) == 0) {
                channel.state.fetch_or(TERMINATED, Relaxed);
                if waker_set {
                    unsafe {
                        (&mut*channel.waker.get()).assume_init_drop();
                    }
                }
                if (state & WAKER_READY) == WAKER_READY {
                    channel.state.fetch_and(!WAKER_READY, Relaxed);
                }
                return Poll::Ready(Err(()));
            }
            if (state & MESSAGE_READY) == MESSAGE_READY {
            fence(Acquire);
            let result = unsafe {
                Ok((&*channel.message.get()).assume_init_read())
            };
            channel.state.fetch_and(!MESSAGE_READY, Relaxed);
            channel.state.fetch_or(TERMINATED, Relaxed);
            if waker_set {
                unsafe {
                    (&mut*channel.waker.get()).assume_init_drop();
                }
            }
            return Poll::Ready(result);
            }
        }
    }
}

impl<T> Drop for InternalReceiver<T> {
    fn drop(&mut self) {
        let channel = unsafe {
            &**self.channel.get()
        };
        let mut state = channel.state.load(Relaxed);
        loop {
            if (state & SENDER) == SENDER {
                match channel.state.compare_exchange(state, state & (!RECEIVER), Relaxed, Relaxed) {
                    Ok(_) => return,
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            }
            fence(Acquire);
            if (state & MESSAGE_READY) == MESSAGE_READY {
                unsafe {
                    (&mut*channel.message.get()).assume_init_drop();
                }
            }
            if (state & WAKER_READY) == WAKER_READY {
                unsafe {
                    (&mut*channel.waker.get()).assume_init_drop();
                }
            }
            unsafe {
                ManuallyDrop::drop(&mut*self.channel.get());
            }
            return;
        }
    }
}

        