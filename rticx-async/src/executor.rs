use core::{
    cell::UnsafeCell,
    future::Future,
    mem::{self, MaybeUninit},
    pin::Pin,
    ptr,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use portable_atomic::{AtomicBool, AtomicPtr, Ordering};

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

pub struct ExecSlot<F: Future<Output = ()> + 'static> {
    future: UnsafeCell<MaybeUninit<F>>,
    running: AtomicBool,
    pending: AtomicBool,
}

unsafe impl<F: Future<Output = ()> + 'static> Sync for ExecSlot<F> {}

impl<F: Future<Output = ()> + 'static> Default for ExecSlot<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: Future<Output = ()> + 'static> ExecSlot<F> {
    pub const fn new() -> Self {
        Self {
            future: UnsafeCell::new(MaybeUninit::uninit()),
            running: AtomicBool::new(false),
            pending: AtomicBool::new(false),
        }
    }

    pub fn new_from_witness<T, I>(_witness: fn(T, I) -> F) -> Self {
        Self::new()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn try_allocate(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// # Safety
    ///
    /// Caller must ensure that `f` has the same concrete type `F` that this
    /// `ExecSlot` was created with. The slot must be in the allocated
    /// (`try_allocate` succeeded) but not yet spawned state.
    pub unsafe fn spawn(&self, f: F) {
        unsafe {
            self.future.get().write(MaybeUninit::new(f));
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
        let future = unsafe { Pin::new_unchecked(&mut *(self.future.get() as *mut F)) };
        match future.poll(&mut cx) {
            Poll::Ready(()) => {
                unsafe {
                    ptr::drop_in_place(self.future.get() as *mut F);
                }
                self.running.store(false, Ordering::Release);
                false
            }
            Poll::Pending => true,
        }
    }
}

pub struct ExecSlotPtr {
    ptr: AtomicPtr<()>,
}

unsafe impl Sync for ExecSlotPtr {}

impl Default for ExecSlotPtr {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecSlotPtr {
    pub const fn new() -> Self {
        Self {
            ptr: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub fn store(&self, p: *const ()) {
        self.ptr.store(p as *mut (), Ordering::Relaxed);
    }

    fn as_ptr(&self) -> *const () {
        self.ptr.load(Ordering::Relaxed)
    }
}

/// # Safety
///
/// The caller must guarantee that the `ExecSlotPtr` was previously stored with
/// a pointer to an `ExecSlot<F>` where `F` is the concrete future type inferred
/// from the witness function. Using the wrong witness function will result in
/// type mismatch and undefined behavior.
pub unsafe fn recover_slot<F: Future<Output = ()> + 'static, T, I>(
    _witness: fn(T, I) -> F,
    ptr: &'static ExecSlotPtr,
) -> &'static ExecSlot<F> {
    unsafe { &*(ptr.as_ptr() as *const ExecSlot<F>) }
}
