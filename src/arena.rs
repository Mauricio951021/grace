


use core::slice;
use std::alloc::{Layout, alloc, handle_alloc_error};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::*, fence};
use std::ptr::dangling_mut;
use std::{mem, u64, usize};
use std::task::{Context, Poll};


pub(crate) const DEFAULT_ARENA_SLOTS: usize = 64;

const BLOCK_SIZE: usize = 64;

const SLOT_SIZE: usize = BLOCK_SIZE * 64;

pub(crate) struct Arena {
    ptr: *mut ArenaInner,
}

impl Arena {
    pub(crate) fn new(mut n: usize) -> Self {
        if n == 0 {
            n = DEFAULT_ARENA_SLOTS;
        }
        ArenaInner::new(n)
    }

    fn mask(&self) -> usize {
        unsafe {
            (*self.ptr).mask
        }
    }

    fn slot_state(&self) -> &SlotState {
        unsafe {
            &(*self.ptr).slot_state
        }
    }

    fn data(&self) -> &ArenaData {
        unsafe {
            &(*self.ptr).data
        }
    }
}

impl Drop for Arena {
    //Esto queda para mas adelante
    fn drop(&mut self) {
        todo!()
    }
}

struct ArenaInner {
    data: ArenaData,
    slot_state: SlotState,
    mask: usize,
}


struct ArenaData{
    ptr: *mut u8,
    len: usize,
}


impl Deref for ArenaData {
    type Target = [[u8; SLOT_SIZE]];
    fn deref(&self) -> &Self::Target {
        unsafe {
            slice::from_raw_parts(self.ptr as *const [u8; SLOT_SIZE], self.len)
        }
    }
}


struct SlotState {
    ptr: *mut MCLock,
    len: usize,
}

impl Deref for SlotState {
    type Target = [MCLock];
    fn deref(&self) -> &Self::Target {
        unsafe {
            slice::from_raw_parts(self.ptr, self.len)
        }
    }
}

impl ArenaInner {
    fn new(size: usize) -> Arena {
        assert!(size.is_power_of_two(), "must be power of two");
        assert!((size * SLOT_SIZE) <= isize::MAX as usize, "too large");
        let arena_layout = Layout::new::<ArenaInner>();
        let (arena_with_data, data_ptr_offset) = arena_layout.extend(Layout::from_size_align(size * SLOT_SIZE, 4096).unwrap()).expect("too large");
        let (mut final_layout, mcl_ptr_offset) = arena_with_data.extend(Layout::array::<MCLock>(size).unwrap()).expect("too large");
        final_layout = final_layout.pad_to_align();
        assert!(final_layout.size() <= isize::MAX as usize, "too large");
        let ptr = unsafe {
            alloc(final_layout) as *mut ArenaInner
        };
        if ptr.is_null() {
            handle_alloc_error(final_layout)
        }
        unsafe {
            (*ptr).data.ptr = (ptr as *mut u8).add(data_ptr_offset) as *mut u8;
            (*ptr).data.len = size;
            (*ptr).slot_state.ptr = (ptr as *mut u8).add(mcl_ptr_offset) as *mut MCLock;
            (*ptr).slot_state.len = size;
            (*ptr).mask = size - 1;
            for i in 0..size {
                (*ptr).slot_state.ptr.add(i).write(MCLock {
                    lock: AtomicU8::new(0),
                    cap: AtomicU8::new(64),
                    bitmap: AtomicU64::new(u64::MAX),
                });
            }
        }
        Arena { ptr }
    }
}


pub(crate) struct ArenaBox<T: ?Sized> {
    pub(crate) data: *mut T,
    drop_mask: u64,
    n_blocks: u8,
    slot_ptr: *const MCLock,
    is_heap: bool,
}

impl <T: ?Sized> ArenaBox<T> {
    pub(crate) fn metadata(&self) -> Metadata {
        Metadata {
            drop_mask: self.drop_mask,
            n_blocks: self.n_blocks,
            slot_ptr: self.slot_ptr,
            is_heap: self.is_heap,
        }
    }

    pub(crate) fn from_raw(ptr: *mut T, data: Metadata) -> Self {
        ArenaBox {
            data: ptr,
            drop_mask: data.drop_mask,
            n_blocks: data.n_blocks,
            slot_ptr: data.slot_ptr,
            is_heap: data.is_heap,
        }
    }
}



unsafe impl<T: Send> Send for ArenaBox<T> {}
unsafe impl<T: Sync> Sync for ArenaBox<T> {}

impl<T: Future> Future for ArenaBox<T> {
    type Output = T::Output;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            Pin::new_unchecked(&mut *self.data).poll(cx)
        }
    }
}

impl<T: ?Sized> Deref for ArenaBox<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            &*self.data
        }
    }
}

impl<T: ?Sized> DerefMut for ArenaBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            &mut *self.data
        }
    }
}

impl<T: ?Sized> Drop for ArenaBox<T> {
    fn drop(&mut self) {
        if self.is_heap {
            unsafe {
                drop(Box::from_raw(self.data));
                return;
            }
        }
        unsafe {
            self.data.drop_in_place();
        }
        if self.n_blocks == 0 {return;}
        let mcl = unsafe {
            &(*self.slot_ptr)
        };
        mcl.bitmap.fetch_or(self.drop_mask, Release);
        mcl.cap.fetch_add(self.n_blocks, Release);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Metadata {
    drop_mask: u64,
    n_blocks: u8,
    slot_ptr: *const MCLock,
    is_heap: bool,
}


//this not have any to do with clocks, it's name means map, cap and lock
#[derive(Debug)]
#[repr(align(64))]
struct MCLock {
    lock: AtomicU8,
    cap: AtomicU8,
    bitmap: AtomicU64,
}



impl Arena {
    
    pub(crate) fn arena_alloc<T>(&self, rand_idx: usize, val: T) -> ArenaBox<T> {
        let size = size_of::<T>();
        if size > SLOT_SIZE {
            return ArenaBox {
                data: Box::into_raw(Box::new(val)),
                drop_mask: 0,
                n_blocks: 0,
                slot_ptr: 0 as *const MCLock,
                is_heap: true,
            };
        }
        if size == 0 {
            mem::forget(val);
            return ArenaBox {
                data: dangling_mut(),
                drop_mask: 0,
                n_blocks: 0,
                slot_ptr: 0 as *const MCLock,
                is_heap: false,
            };
        }
        let align = align_of::<T>();

        let n_blocks = ((size + BLOCK_SIZE - 1) / BLOCK_SIZE) as u8;
        let start_idx = rand_idx & self.mask();
        let mut idx = (start_idx + 1) & self.mask();
        let mut loop_count: u8 = 0;
        
        loop {

            if idx == start_idx {
                loop_count += 1;
                if loop_count == 2 {
                    break;
                }
            } 

            let state = &self.slot_state()[idx];

            if state.lock.swap(1, Relaxed) == 1 {
                idx = (idx + 1) & self.mask();
                continue;
            }
            if state.cap.load(Acquire) < n_blocks {
                state.lock.store(0, Relaxed);
                idx = (idx + 1) & self.mask();
                continue;
            }
            if let Some(pos) = find_blocks(state.bitmap.load(Relaxed), n_blocks, align) {
                fence(Acquire);
                let mask: u64 = (1 << n_blocks) - 1 << pos;
                let ptr;
                unsafe {
                    ptr = self.data()[idx].as_ptr().add(pos * BLOCK_SIZE) as *mut T;
                    ptr.write(val); 
                }
                state.bitmap.fetch_and(!mask, Release);
                state.cap.fetch_sub(n_blocks, Release);
                state.lock.store(0, Relaxed);
                return ArenaBox {
                    data: ptr,
                    drop_mask: mask,
                    n_blocks,
                    slot_ptr: state as *const MCLock,
                    is_heap: false,
                };
            }
            idx = (idx + 1) & self.mask();
        }
        ArenaBox {
            data: Box::into_raw(Box::new(val)),
            drop_mask: 0,
            n_blocks: 0,
            slot_ptr: 0 as *const MCLock,
            is_heap: true,
        }
    }
}


//los bit en 1 significan bloques librea
//esta funcion devuelve el indice donde empiezan n_blocks contiguos libres o None
fn find_blocks(mut map: u64, n_blocks: u8, align: usize) -> Option<usize> {
    //quien llama ya verifico capacidad sufiente y este es el caso maximo asi que el slot esta vacio
    if n_blocks == 64 {
        return Some(0);
    }
    
    for _ in 0..n_blocks - 1 {
        map &= map >> 1;
        if map == 0 {return None;}
    }
    if align <= BLOCK_SIZE {return Some(map.trailing_zeros() as usize)}
    map &= u64::MAX / ((1 << align / BLOCK_SIZE) - 1);
    if map != 0 {
        Some(map.trailing_zeros() as usize)
    } else {
        None
    }
}
