use alloc::boxed::Box;
use core::{
    cell::UnsafeCell,
    future::Future,
    mem,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use portable_atomic::{AtomicBool, Ordering};

static WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(waker_clone, waker_wake, waker_wake, waker_drop);

unsafe fn waker_clone(p: *const ()) -> RawWaker {
    RawWaker::new(p, &WAKER_VTABLE)
}

unsafe fn waker_wake(p: *const ()) {
    let f: fn() = unsafe { mem::transmute(p) };
    f();
}

unsafe fn waker_drop(_: *const ()) {}

pub struct ExecSlot {
    future: UnsafeCell<Option<Pin<Box<dyn Future<Output = ()> + 'static>>>>,
    running: AtomicBool,
    pending: AtomicBool,
}

unsafe impl Sync for ExecSlot {}

impl ExecSlot {
    pub const fn new() -> Self {
        Self {
            future: UnsafeCell::new(None),
            running: AtomicBool::new(false),
            pending: AtomicBool::new(false),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn try_allocate(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    pub unsafe fn install<F: Future<Output = ()> + 'static>(&self, f: F) {
        unsafe {
            (*self.future.get()) = Some(Box::pin(f));
        }
        self.set_pending();
    }

    pub fn set_pending(&self) {
        self.pending.store(true, Ordering::Release);
    }

    fn check_and_clear_pending(&self) -> bool {
        self.pending
            .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn waker(&self, wake: fn()) -> Waker {
        unsafe { Waker::from_raw(RawWaker::new(wake as *const (), &WAKER_VTABLE)) }
    }

    pub fn poll(&self, wake: fn()) -> bool {
        if !self.is_running() {
            return false;
        }
        if !self.check_and_clear_pending() {
            return true;
        }
        let waker = self.waker(wake);
        let mut cx = Context::from_waker(&waker);
        let future = unsafe { &mut *self.future.get() };
        match future
            .as_mut()
            .expect("running slot must have a future")
            .as_mut()
            .poll(&mut cx)
        {
            Poll::Ready(()) => {
                unsafe {
                    (*self.future.get()) = None;
                }
                self.running.store(false, Ordering::Release);
                false
            }
            Poll::Pending => true,
        }
    }
}
