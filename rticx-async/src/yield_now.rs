use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct YieldNow(bool);

impl Default for YieldNow {
    fn default() -> Self {
        Self::new()
    }
}

impl YieldNow {
    pub fn new() -> Self {
        YieldNow(false)
    }
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

pub async fn yield_now() {
    YieldNow::new().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::pin;

    #[test]
    fn yield_now_completes() {
        // Drive the future manually to verify it completes after one extra poll
        let mut fut = pin!(YieldNow::new());
        let waker = core::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        // First poll: returns Pending
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        // Second poll: returns Ready(())
        assert!(fut.as_mut().poll(&mut cx).is_ready());
    }
}
