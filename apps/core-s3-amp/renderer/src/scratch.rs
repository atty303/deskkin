// SPDX-License-Identifier: GPL-3.0-only

use core::{
    alloc::Layout,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

extern "C" {
    fn deskkin_scratch_alloc(size: usize, alignment: usize) -> *mut u8;
    fn deskkin_scratch_free(block: *mut u8);
}

/// Renderer-owned hot state allocated from APPCPU's internal SRAM heap.
pub struct Scratch<T> {
    pointer: NonNull<T>,
    initialized: usize,
}

impl<T: Default> Scratch<T> {
    pub fn new(len: usize) -> Self {
        let layout = Layout::array::<T>(len).expect("scratch layout");
        assert!(layout.size() != 0);
        let pointer = NonNull::new(unsafe {
            deskkin_scratch_alloc(layout.size(), layout.align()).cast::<T>()
        })
        .expect("internal renderer scratch allocation");
        let mut result = Self {
            pointer,
            initialized: 0,
        };
        for index in 0..len {
            unsafe { result.pointer.as_ptr().add(index).write(T::default()) };
            result.initialized += 1;
        }
        result
    }
}

impl<T> Deref for Scratch<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { core::slice::from_raw_parts(self.pointer.as_ptr(), self.initialized) }
    }
}

impl<T> DerefMut for Scratch<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { core::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.initialized) }
    }
}

impl<T> Drop for Scratch<T> {
    fn drop(&mut self) {
        unsafe {
            core::ptr::drop_in_place(core::ptr::slice_from_raw_parts_mut(
                self.pointer.as_ptr(),
                self.initialized,
            ));
            deskkin_scratch_free(self.pointer.as_ptr().cast());
        }
    }
}
