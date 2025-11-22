#![no_std]

use core::mem::MaybeUninit;

pub struct Queue<T: Clone, const DEPTH: usize> {
    buffer: [MaybeUninit<T>; DEPTH],
    read_idx: usize,
    write_idx: usize,
}

impl<T: Clone, const DEPTH: usize> Queue<T, DEPTH> {
    #[inline(always)]
    pub const fn new() -> Self {
        let buffer = unsafe { MaybeUninit::zeroed().assume_init() };
        Queue {
            buffer,
            read_idx: 0,
            write_idx: 0,
        }
    }

    pub fn enqueue(&mut self, data: T) -> Result<(), T> {
        let w = self.write_idx;

        if ((w + 1) % DEPTH) != (self.read_idx % DEPTH) {
            self.buffer[w % DEPTH].write(data);
            self.write_idx += 1;
            Ok(())
        } else {
            Err(data)
        }
    }

    /// Adds an `item` to the end of the queue, without checking if it's full
    ///
    /// # Safety
    ///
    /// If the queue is full this operation will leak a value (T's destructor won't run on
    /// the value that got overwritten by `item`), *and* will allow the `dequeue` operation
    /// to create a copy of `item`, which could result in `T`'s destructor running on `item`
    /// twice.
    pub unsafe fn enqueue_unchecked(&mut self, data: T) {
        self.buffer[self.write_idx % DEPTH].write(data);
        self.write_idx += 1;
    }

    pub fn dequeue(&mut self) -> Option<T> {
        let w = self.write_idx % DEPTH;
        let r = self.read_idx % DEPTH;
        if r == w {
            None
        } else {
            let data = unsafe { self.buffer[r].assume_init_read() };
            self.read_idx += 1;
            Some(data)
        }
    }

    /// Returns the item in the front of the queue, without checking if there is something in the
    /// queue
    ///
    /// # Safety
    ///
    /// If the queue is empty this operation will return uninitialized memory.
    pub unsafe fn dequeue_unchecked(&mut self) -> T {
        let data = unsafe { self.buffer[self.read_idx % DEPTH].assume_init_read() };
        self.read_idx += 1;
        data
    }

    // Splits a queue into producer and consumer endpoints
    pub fn split(&mut self) -> (Producer<'_, T, DEPTH>, Consumer<'_, T, DEPTH>) {
        let self1 = unsafe { (self as *mut Queue<T, DEPTH>).as_mut().unwrap() };
        let self2 = unsafe { (self as *mut Queue<T, DEPTH>).as_mut().unwrap() };
        // The reason why we can do this is because we can guarantee that Producer can mutate only the buffer and write_index
        // while the Consumer only mutates read_index
        (Producer { q: self1 }, Consumer { q: self2 })
    }
}

pub struct Producer<'a, T: Clone, const DEPTH: usize> {
    q: &'a mut Queue<T, DEPTH>,
}

impl<'a, T: Clone, const DEPTH: usize> Producer<'a, T, DEPTH> {
    pub fn enqueue(&mut self, data: T) -> Result<(), T> {
        self.q.enqueue(data)
    }

    /// Adds an `item` to the end of the queue, without checking if it's full
    ///
    /// # Safety
    ///
    /// If the queue is full this operation will leak a value (T's destructor won't run on
    /// the value that got overwritten by `item`), *and* will allow the `dequeue` operation
    /// to create a copy of `item`, which could result in `T`'s destructor running on `item`
    /// twice.
    pub unsafe fn enqueue_unchecked(&mut self, data: T) {
        unsafe { self.q.enqueue_unchecked(data) }
    }
}

pub struct Consumer<'a, T: Clone, const DEPTH: usize> {
    q: &'a mut Queue<T, DEPTH>,
}

impl<'a, T: Clone, const DEPTH: usize> Consumer<'a, T, DEPTH> {
    pub fn dequeue(&mut self) -> Option<T> {
        self.q.dequeue()
    }

    /// Returns the item in the front of the queue, without checking if there is something in the
    /// queue
    ///
    /// # Safety
    ///
    /// If the queue is empty this operation will return uninitialized memory.
    pub unsafe fn dequeue_unchecked(&mut self) -> T {
        unsafe { self.q.dequeue_unchecked() }
    }
}

// unsafe impl<T: Clone, const DEPTH: usize> Sync for Queue<T, DEPTH> {}

// tests

#[cfg(test)]
mod tests {
    use crate::Queue;

    #[test]
    fn test_queue1() {
        let mut q: Queue<u32, 3> = Queue::new();
        assert!(q.enqueue(1).is_ok());
        assert!(q.enqueue(2).is_ok());
        assert!(q.enqueue(3).is_err()); // In a ring buffer implementation with a read/write index, one element is always unused
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.dequeue(), Some(2));
        assert_eq!(q.dequeue(), None);
        assert!(q.enqueue(4).is_ok());
        assert!(q.enqueue(5).is_ok());
        assert!(q.enqueue(6).is_err());
        assert_eq!(q.dequeue(), Some(4));
        assert_eq!(q.dequeue(), Some(5));
        assert!(q.enqueue(7).is_ok());
        assert_eq!(q.dequeue(), Some(7));
        assert_eq!(q.dequeue(), None);
        assert!(q.enqueue(8).is_ok());
        assert!(q.enqueue(9).is_ok());
        assert_eq!(q.dequeue(), Some(8));
        assert_eq!(q.dequeue(), Some(9));
        assert_eq!(q.dequeue(), None);
    }

    #[test]
    fn test_queue2() {
        let mut q: Queue<u32, 3> = Queue::new();
        let (mut p, mut c) = q.split();
        assert!(p.enqueue(1).is_ok());
        assert!(p.enqueue(2).is_ok());
        assert!(p.enqueue(3).is_err()); // In a ring buffer implementation with a read/write index, one element is always unused
        assert_eq!(c.dequeue(), Some(1));
        assert_eq!(c.dequeue(), Some(2));
        assert_eq!(c.dequeue(), None);
        assert!(p.enqueue(4).is_ok());
        assert!(p.enqueue(5).is_ok());
        assert!(p.enqueue(6).is_err());
        assert_eq!(c.dequeue(), Some(4));
        assert_eq!(c.dequeue(), Some(5));
        assert!(p.enqueue(7).is_ok());
        assert_eq!(c.dequeue(), Some(7));
        assert_eq!(c.dequeue(), None);
        assert!(p.enqueue(8).is_ok());
        assert!(p.enqueue(9).is_ok());
        assert_eq!(q.dequeue(), Some(8));
        assert_eq!(q.dequeue(), Some(9));
        assert_eq!(q.dequeue(), None);
    }
}
