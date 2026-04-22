

use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, Ordering::*, fence};





pub struct OneShot<T> {
    state: AtomicU8,
    message: MaybeUninit<T>,
    waker: MaybeUninit<Waker>,
}

// the sender is alive
const SENDER: u8 = 1;

//the receiver is alive
const RECEIVER: u8 = 2;

const MESSAGE_READY: u8 = 4;

const WAKER_READY: u8 = 8;

const TERMINATED: u8 = 16;


impl<T> OneShot<T> {
    pub fn new() -> (Sender<T>, Receiver<T>)
    where 
    T: Send + 'static,
    {
        let msg = OneShot {
            state: AtomicU8::new(SENDER | RECEIVER),
            message: MaybeUninit::uninit(),
            waker: MaybeUninit::uninit(),
        };
        let channel = Box::into_raw(Box::new(msg));
        let sender = Sender {channel};
        let receiver = Receiver {channel};
        (sender, receiver)
    }
}

pub struct Sender<T> {
    channel: *mut OneShot<T>,
}

unsafe impl<T: Send> Send for Sender<T> {}


impl<T> Sender<T> 
where 
T: Send + 'static,
{
    pub fn send(self, msg: T) {
        let channel = unsafe {
            &*self.channel
        };
        let mut state = channel.state.load(Relaxed);
        if (state & RECEIVER) == RECEIVER {
            unsafe {
                (*self.channel).message.write(msg);
            }
            state = channel.state.fetch_or(MESSAGE_READY, Release);
            if (state & WAKER_READY) == WAKER_READY {
                channel.state.fetch_and(!WAKER_READY, Acquire);
                unsafe {
                    (*self.channel).waker.assume_init_read().wake();
                }   
            }
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let channel = unsafe {
            &*self.channel
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
            if (state & MESSAGE_READY) == MESSAGE_READY  {
                unsafe {
                    (*self.channel).message.assume_init_drop();
                }
            }
            if (state & WAKER_READY) == WAKER_READY {
                fence(Acquire);
                unsafe {
                    (*self.channel).waker.assume_init_drop();
                }
            }
            unsafe {
                drop(Box::from_raw(self.channel));
            }
            return;
        }
    }
}


pub struct Receiver<T> {
    channel: *mut OneShot<T>,
}

unsafe impl<T: Send> Send for Receiver<T> {}

impl<T> Future for Receiver<T> {
    type Output = Result<T, ()>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let channel = unsafe {
            &*self.channel
        };
        let mut state = channel.state.load(Relaxed);
        if (state & TERMINATED) == TERMINATED {
            panic!("can not poll after return Ready");
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
                    if (*self.channel).waker.assume_init_ref().will_wake(cx.waker()) {
                        channel.state.fetch_or(WAKER_READY, Release);
                        true
                    } else {
                        (*self.channel).waker.assume_init_drop();
                        false
                    }
                }
            } else {
                //Here i put true because Sender::send already write the message and turn off WAKER_READY and consume the Waker
                //actually will be the same true or false
                true
            }
        } else {
            false
        };

        if (state & MESSAGE_READY) == MESSAGE_READY {
            fence(Acquire);
            let result = unsafe {
                Ok((*self.channel).message.assume_init_read())
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
                        (*self.channel).waker.write(waker);
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
                        (*self.channel).waker.assume_init_drop();
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
                Ok((*self.channel).message.assume_init_read())
            };
            channel.state.fetch_and(!MESSAGE_READY, Relaxed);
            channel.state.fetch_or(TERMINATED, Relaxed);
            if waker_set {
                unsafe {
                    (*self.channel).waker.assume_init_drop();
                }
            }
            return Poll::Ready(result);
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let channel = unsafe {
            &*self.channel
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
            if (state & MESSAGE_READY) == MESSAGE_READY  {
                fence(Acquire);
                unsafe {
                    (*self.channel).message.assume_init_drop();
                }
            }
            if (state & WAKER_READY) == WAKER_READY {
                fence(Acquire);
                unsafe {
                    (*self.channel).waker.assume_init_drop();
                }
            }
            unsafe {
                drop(Box::from_raw(self.channel));
            }
            return;
        }
    }
}