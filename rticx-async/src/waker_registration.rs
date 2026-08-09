use core::cell::UnsafeCell;
use core::task::Waker;

pub struct CriticalSectionWakerRegistration {
    waker: UnsafeCell<Option<Waker>>,
}

unsafe impl Send for CriticalSectionWakerRegistration {}
unsafe impl Sync for CriticalSectionWakerRegistration {}

impl CriticalSectionWakerRegistration {
    pub const fn new() -> Self {
        Self {
            waker: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, new_waker: &Waker) {
        critical_section::with(|_| {
            let self_waker = unsafe { &mut *self.waker.get() };

            match self_waker {
                Some(w2) if w2.will_wake(new_waker) => {}
                _ => {
                    if let Some(old_waker) = self_waker.replace(new_waker.clone()) {
                        old_waker.wake()
                    }
                }
            }
        });
    }

    pub fn wake(&self) {
        critical_section::with(|_| {
            let self_waker = unsafe { &mut *self.waker.get() };
            if let Some(waker) = self_waker.take() {
                waker.wake()
            }
        });
    }
}
