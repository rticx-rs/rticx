use core::{
    cell::UnsafeCell,
    future::poll_fn,
    mem::MaybeUninit,
    pin::Pin,
    ptr,
    sync::atomic::{fence, Ordering},
    task::{Poll, Waker},
};
#[doc(hidden)]
pub use critical_section;
use heapless::Deque;

use crate::{
    dropper::OnDrop,
    wait_queue::{DoublyLinkedList, Link},
    waker_registration::CriticalSectionWakerRegistration as WakerRegistration,
};

type WaitQueueData = (Waker, SlotPtr);
type WaitQueue = DoublyLinkedList<WaitQueueData>;

pub struct Channel<T, const N: usize> {
    freeq: UnsafeCell<Deque<u8, N>>,
    readyq: UnsafeCell<Deque<u8, N>>,
    receiver_waker: WakerRegistration,
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    wait_queue: WaitQueue,
    receiver_dropped: UnsafeCell<bool>,
    num_senders: UnsafeCell<usize>,
}

unsafe impl<T: Send, const N: usize> Send for Channel<T, N> {}
unsafe impl<T: Send, const N: usize> Sync for Channel<T, N> {}

macro_rules! cs_access {
    ($name:ident, $type:ty) => {
        unsafe fn $name<F, R>(&self, _cs: critical_section::CriticalSection, f: F) -> R
        where
            F: FnOnce(&mut $type) -> R,
        {
            let v = self.$name.get();
            let v = unsafe { &mut *v };
            f(v)
        }
    };
}

impl<T, const N: usize> Channel<T, N> {
    const fn size_check() {
        assert!(N < 256, "This queue supports a maximum of 255 entries");
    }

    pub const fn new() -> Self {
        const { Self::size_check() };
        Self {
            freeq: UnsafeCell::new(Deque::new()),
            readyq: UnsafeCell::new(Deque::new()),
            receiver_waker: WakerRegistration::new(),
            slots: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            wait_queue: WaitQueue::new(),
            receiver_dropped: UnsafeCell::new(false),
            num_senders: UnsafeCell::new(0),
        }
    }

    pub fn split(&mut self) -> (Sender<'_, T, N>, Receiver<'_, T, N>) {
        debug_assert!(unsafe { &mut *self.readyq.get() }.is_empty());

        let freeq = unsafe { &mut *self.freeq.get() };
        freeq.clear();

        for idx in 0..N as u8 {
            debug_assert!(!freeq.is_full());
            unsafe { freeq.push_back_unchecked(idx) };
        }

        debug_assert!(freeq.is_full());

        *unsafe { &mut *self.num_senders.get() } = 1;

        (Sender(self), Receiver(self))
    }

    cs_access!(freeq, Deque<u8, N>);
    cs_access!(readyq, Deque<u8, N>);
    cs_access!(receiver_dropped, bool);
    cs_access!(num_senders, usize);

    unsafe fn return_free_slot(&self, slot: u8) {
        critical_section::with(|cs| {
            fence(Ordering::SeqCst);

            if let Some((wait_head, mut freeq_slot)) = self.wait_queue.pop() {
                unsafe { freeq_slot.replace(Some(slot), cs) };
                wait_head.wake();
            } else {
                unsafe {
                    self.freeq(cs, |freeq| {
                        debug_assert!(!freeq.is_full());
                        freeq.push_back_unchecked(slot);
                    });
                }
            }
        })
    }

    unsafe fn read_slot(&self, slot: u8) -> T {
        let first_element = unsafe { self.slots.get_unchecked(slot as usize) }.get();
        let ptr = unsafe { &mut *first_element }.as_mut_ptr();
        unsafe { ptr::read(ptr) }
    }
}

// -------- Sender

pub struct NoReceiver<T>(pub T);

pub enum TrySendError<T> {
    NoReceiver(T),
    Full(T),
}

impl<T> core::fmt::Debug for NoReceiver<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "NoReceiver({:?})", self.0)
    }
}

impl<T> core::fmt::Debug for TrySendError<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TrySendError::NoReceiver(v) => write!(f, "NoReceiver({v:?})"),
            TrySendError::Full(v) => write!(f, "Full({v:?})"),
        }
    }
}

impl<T> PartialEq for TrySendError<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TrySendError::NoReceiver(v1), TrySendError::NoReceiver(v2)) => v1.eq(v2),
            (TrySendError::Full(v1), TrySendError::Full(v2)) => v1.eq(v2),
            _ => false,
        }
    }
}

pub struct Sender<'a, T, const N: usize>(&'a Channel<T, N>);

#[derive(Clone)]
struct LinkPtr(*mut Option<Link<WaitQueueData>>);

impl LinkPtr {
    unsafe fn get(&mut self) -> &mut Option<Link<WaitQueueData>> {
        unsafe { &mut *self.0 }
    }
}

unsafe impl Send for LinkPtr {}
unsafe impl Sync for LinkPtr {}

#[derive(Clone)]
struct SlotPtr(*mut Option<u8>);

impl SlotPtr {
    unsafe fn replace(
        &mut self,
        new_value: Option<u8>,
        _cs: critical_section::CriticalSection,
    ) -> Option<u8> {
        unsafe { core::ptr::replace(self.0, new_value) }
    }

    unsafe fn replace_exclusive(&mut self, new_value: Option<u8>) -> Option<u8> {
        unsafe { core::ptr::replace(self.0, new_value) }
    }
}

unsafe impl Send for SlotPtr {}
unsafe impl Sync for SlotPtr {}

impl<T, const N: usize> Sender<'_, T, N> {
    #[inline(always)]
    fn send_footer(&mut self, idx: u8, val: T) {
        unsafe {
            let first_element = self.0.slots.get_unchecked(idx as usize).get();
            let ptr = (&mut *first_element).as_mut_ptr();
            ptr::write(ptr, val)
        }

        critical_section::with(|cs| {
            unsafe {
                self.0.readyq(cs, |readyq| {
                    debug_assert!(!readyq.is_full());
                    readyq.push_back_unchecked(idx);
                });
            }
        });

        fence(Ordering::SeqCst);
        self.0.receiver_waker.wake();
    }

    pub fn try_send(&mut self, val: T) -> Result<(), TrySendError<T>> {
        if !self.0.wait_queue.is_empty() {
            return Err(TrySendError::Full(val));
        }

        if self.is_closed() {
            return Err(TrySendError::NoReceiver(val));
        }

        let free_slot = critical_section::with(|cs| unsafe {
            self.0.freeq(cs, |q| q.pop_front())
        });

        let idx = if let Some(idx) = free_slot {
            idx
        } else {
            return Err(TrySendError::Full(val));
        };

        self.send_footer(idx, val);
        Ok(())
    }

    pub async fn send(&mut self, val: T) -> Result<(), NoReceiver<T>> {
        let mut free_slot_ptr: Option<u8> = None;
        let mut link_ptr: Option<Link<WaitQueueData>> = None;

        let mut link_ptr = LinkPtr(core::ptr::addr_of_mut!(link_ptr));
        let mut free_slot_ptr = SlotPtr(core::ptr::addr_of_mut!(free_slot_ptr));

        let mut link_ptr2 = link_ptr.clone();
        let mut free_slot_ptr2 = free_slot_ptr.clone();
        let dropper = OnDrop::new(|| {
            if let Some(link) = unsafe { link_ptr2.get() } {
                link.remove_from_list(&self.0.wait_queue);
            }
            if let Some(freed_slot) = unsafe { free_slot_ptr2.replace_exclusive(None) } {
                unsafe { self.0.return_free_slot(freed_slot) };
            }
        });

        let idx = poll_fn(|cx| {
            critical_section::with(|cs| {
                if self.is_closed() {
                    return Poll::Ready(Err(()));
                }

                let wq_empty = self.0.wait_queue.is_empty();
                let freeq_empty = unsafe { self.0.freeq(cs, |q| q.is_empty()) };

                let link = unsafe { link_ptr.get() };

                if let Some(queue_link) = link {
                    if queue_link.is_popped() {
                        let slot = unsafe { free_slot_ptr.replace(None, cs) };
                        link.take();
                        if let Some(slot) = slot {
                            Poll::Ready(Ok(slot))
                        } else {
                            Poll::Ready(Err(()))
                        }
                    } else {
                        Poll::Pending
                    }
                } else if !wq_empty || freeq_empty {
                    let link_ref =
                        link.insert(Link::new((cx.waker().clone(), free_slot_ptr.clone())));
                    unsafe { self.0.wait_queue.push(Pin::new_unchecked(link_ref)) };
                    Poll::Pending
                } else {
                    unsafe {
                        self.0.freeq(cs, |freeq| {
                            debug_assert!(!freeq.is_empty());
                            let slot = freeq.pop_back_unchecked();
                            Poll::Ready(Ok(slot))
                        })
                    }
                }
            })
        })
        .await;

        drop(dropper);

        if let Ok(idx) = idx {
            self.send_footer(idx, val);
            Ok(())
        } else {
            Err(NoReceiver(val))
        }
    }

    pub fn is_closed(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.receiver_dropped(cs, |v| *v) })
    }

    pub fn is_full(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.freeq(cs, |v| v.is_empty()) })
    }

    pub fn is_empty(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.freeq(cs, |v| v.is_full()) })
    }
}

impl<T, const N: usize> Drop for Sender<'_, T, N> {
    fn drop(&mut self) {
        let num_senders = critical_section::with(|cs| unsafe {
            self.0.num_senders(cs, |s| {
                *s -= 1;
                *s
            })
        });

        if num_senders == 0 {
            self.0.receiver_waker.wake();
        }
    }
}

impl<T, const N: usize> Clone for Sender<'_, T, N> {
    fn clone(&self) -> Self {
        critical_section::with(|cs| unsafe {
            self.0.num_senders(cs, |v| *v += 1);
        });
        Self(self.0)
    }
}

impl<T, const N: usize> core::fmt::Debug for Sender<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Sender")
    }
}

// -------- Receiver

pub struct Receiver<'a, T, const N: usize>(&'a Channel<T, N>);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ReceiveError {
    NoSender,
    Empty,
}

impl<T, const N: usize> Receiver<'_, T, N> {
    pub fn try_recv(&mut self) -> Result<T, ReceiveError> {
        let ready_slot = critical_section::with(|cs| unsafe {
            self.0.readyq(cs, |q| q.pop_front())
        });

        if let Some(rs) = ready_slot {
            let r = unsafe { self.0.read_slot(rs) };
            unsafe { self.0.return_free_slot(rs) };
            Ok(r)
        } else if self.is_closed() {
            Err(ReceiveError::NoSender)
        } else {
            Err(ReceiveError::Empty)
        }
    }

    pub async fn recv(&mut self) -> Result<T, ReceiveError> {
        poll_fn(|cx| {
            self.0.receiver_waker.register(cx.waker());

            match self.try_recv() {
                Ok(val) => Poll::Ready(Ok(val)),
                Err(ReceiveError::NoSender) => Poll::Ready(Err(ReceiveError::NoSender)),
                _ => Poll::Pending,
            }
        })
        .await
    }

    pub fn is_closed(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.num_senders(cs, |v| *v == 0) })
    }

    pub fn is_full(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.readyq(cs, |v| v.is_full()) })
    }

    pub fn is_empty(&self) -> bool {
        critical_section::with(|cs| unsafe { self.0.readyq(cs, |v| v.is_empty()) })
    }
}

impl<T, const N: usize> Drop for Receiver<'_, T, N> {
    fn drop(&mut self) {
        critical_section::with(|cs| unsafe {
            self.0.receiver_dropped(cs, |v| *v = true);
        });

        let ready_slot = || {
            critical_section::with(|cs| unsafe { self.0.readyq(cs, |q| q.pop_back()) })
        };

        while let Some(slot) = ready_slot() {
            drop(unsafe { self.0.read_slot(slot) })
        }

        while let Some((waker, _)) = self.0.wait_queue.pop() {
            waker.wake();
        }
    }
}

impl<T, const N: usize> core::fmt::Debug for Receiver<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Receiver")
    }
}
