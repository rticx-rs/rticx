use core::{
    future::poll_fn,
    marker::PhantomPinned,
    pin::{Pin, pin},
    ptr::null_mut,
    sync::atomic::Ordering,
    task::{Poll, Waker},
};
pub use critical_section;
use portable_atomic::{AtomicBool, AtomicPtr};

use crate::dropper::OnDropWith;

pub type WaitQueue = DoublyLinkedList<Waker>;

pub struct DoublyLinkedList<T> {
    head: AtomicPtr<Link<T>>,
    tail: AtomicPtr<Link<T>>,
}

impl<T> DoublyLinkedList<T> {
    pub const fn new() -> Self {
        Self {
            head: AtomicPtr::new(null_mut()),
            tail: AtomicPtr::new(null_mut()),
        }
    }
}

impl<T> Default for DoublyLinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> DoublyLinkedList<T> {
    const R: Ordering = Ordering::Relaxed;

    pub fn pop(&self) -> Option<T> {
        critical_section::with(|_| {
            core::sync::atomic::fence(Ordering::SeqCst);

            let head = self.head.load(Self::R);

            if let Some(head_ref) = unsafe { head.as_ref() } {
                self.head.store(head_ref.next.load(Self::R), Self::R);

                let head_val = head_ref.val.clone();

                let tail = self.tail.load(Self::R);
                if head == tail {
                    self.tail.store(null_mut(), Self::R);
                }

                if let Some(next_ref) = unsafe { head_ref.next.load(Self::R).as_ref() } {
                    next_ref.prev.store(null_mut(), Self::R);
                }

                head_ref.next.store(null_mut(), Self::R);
                head_ref.prev.store(null_mut(), Self::R);
                head_ref.is_popped.store(true, Self::R);

                return Some(head_val);
            }

            None
        })
    }

    pub unsafe fn push(&self, link: Pin<&Link<T>>) {
        critical_section::with(|_| {
            core::sync::atomic::fence(Ordering::SeqCst);

            let tail = self.tail.load(Self::R);
            let link = link.get_ref();

            if let Some(tail_ref) = unsafe { tail.as_ref() } {
                link.prev.store(tail, Self::R);
                self.tail.store(link as *const _ as *mut _, Self::R);
                tail_ref.next.store(link as *const _ as *mut _, Self::R);
            } else {
                self.tail.store(link as *const _ as *mut _, Self::R);
                self.head.store(link as *const _ as *mut _, Self::R);
            }
        });
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Self::R).is_null()
    }
}

pub struct Link<T> {
    pub(crate) val: T,
    next: AtomicPtr<Link<T>>,
    prev: AtomicPtr<Link<T>>,
    is_popped: AtomicBool,
    _up: PhantomPinned,
}

impl<T: Clone> Link<T> {
    const R: Ordering = Ordering::Relaxed;

    pub const fn new(val: T) -> Self {
        Self {
            val,
            next: AtomicPtr::new(null_mut()),
            prev: AtomicPtr::new(null_mut()),
            is_popped: AtomicBool::new(false),
            _up: PhantomPinned,
        }
    }

    pub fn is_popped(&self) -> bool {
        self.is_popped.load(Self::R)
    }

    pub fn remove_from_list(&self, list: &DoublyLinkedList<T>) {
        critical_section::with(|_| {
            core::sync::atomic::fence(Ordering::SeqCst);

            if self.is_popped() {
                return;
            }

            let prev = self.prev.load(Self::R);
            let next = self.next.load(Self::R);
            self.is_popped.store(true, Self::R);

            match unsafe { (prev.as_ref(), next.as_ref()) } {
                (None, None) => {
                    let sp = self as *const _;
                    if sp == list.head.load(Ordering::Relaxed) {
                        list.head.store(null_mut(), Self::R);
                        list.tail.store(null_mut(), Self::R);
                    }
                }
                (None, Some(next_ref)) => {
                    next_ref.prev.store(null_mut(), Self::R);
                    list.head.store(next, Self::R);
                }
                (Some(prev_ref), None) => {
                    prev_ref.next.store(null_mut(), Self::R);
                    list.tail.store(prev, Self::R);
                }
                (Some(prev_ref), Some(next_ref)) => {
                    prev_ref.next.store(next, Self::R);
                    next_ref.prev.store(prev, Self::R);
                }
            }
        })
    }
}

impl DoublyLinkedList<Waker> {
    pub async fn wait_until<T, F: FnMut() -> Option<T>>(&self, mut f: F) -> T {
        let link_place = pin!(None::<Link<Waker>>);

        let mut link_guard = OnDropWith::new(link_place, |link| {
            if let Some(link) = link.as_ref().as_pin_ref() {
                link.remove_from_list(self);
            }
            link.set(None);
        });

        poll_fn(move |cx| {
            link_guard.execute();

            if let Some(val) = f() {
                return Poll::Ready(val);
            }

            let new_link = Link::new(cx.waker().clone());

            link_guard.set(Some(new_link));

            let new_link_pinned = link_guard.as_ref().as_pin_ref().expect("We just set it");

            unsafe { self.push(new_link_pinned) };

            Poll::Pending
        })
        .await
    }
}
