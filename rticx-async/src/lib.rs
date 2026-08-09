#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod channel;
mod dropper;
pub mod executor;
mod wait_queue;
mod waker_registration;

pub use portable_atomic;

#[macro_export]
macro_rules! make_channel {
    ($type:ty, $size:expr) => {{
        static mut CHANNEL: $crate::channel::Channel<$type, $size> =
            $crate::channel::Channel::new();

        static CHECK: $crate::portable_atomic::AtomicU8 =
            $crate::portable_atomic::AtomicU8::new(0);

        $crate::channel::critical_section::with(|_| {
            if CHECK.load($crate::portable_atomic::Ordering::Relaxed) != 0 {
                ::core::panic!("call to the same `make_channel` instance twice");
            }
            CHECK.store(1, $crate::portable_atomic::Ordering::Relaxed);
        });

        #[allow(static_mut_refs)]
        unsafe {
            CHANNEL.split()
        }
    }};
}
