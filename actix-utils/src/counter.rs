use core::cell::{Cell, RefCell};
use core::task::{Context, Waker};

use std::collections::LinkedList;
use std::rc::Rc;

/// Simple counter with ability to notify task on reaching specific number
///
/// Counter could be cloned, total n-count is shared across all clones.
pub struct Counter(Rc<CounterInner>);

impl Clone for Counter {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

struct CounterInner {
    count: Cell<usize>,
    capacity: usize,
    have_waiter: Cell<bool>,
    waker: RefCell<LinkedList<Waker>>,
}

impl Counter {
    /// Create `Counter` instance and set max value.
    pub fn new(capacity: usize) -> Self {
        Counter(Rc::new(CounterInner {
            capacity,
            count: Cell::new(0),
            have_waiter: Cell::new(false),
            waker: RefCell::new(LinkedList::new()),
        }))
    }

    /// Get counter guard.
    ///
    /// This call should only be used after `Counter::available` returns
    /// true.
    pub fn get(&self) -> CounterGuard {
        CounterGuard::new(self.0.clone())
    }

    /// Check if counter is not at capacity. If counter at capacity
    /// it registers notification for current task.
    pub fn available(&self, cx: &mut Context<'_>) -> bool {
        self.0.available(cx)
    }

    /// Get total number of acquired counts
    pub fn total(&self) -> usize {
        self.0.count.get()
    }
}

pub struct CounterGuard(Rc<CounterInner>);

impl CounterGuard {
    fn new(inner: Rc<CounterInner>) -> Self {
        inner.inc();
        CounterGuard(inner)
    }
}

impl Unpin for CounterGuard {}

impl Drop for CounterGuard {
    fn drop(&mut self) {
        self.0.dec();
    }
}

impl CounterInner {
    fn inc(&self) {
        self.count.set(self.count.get() + 1);
    }

    fn dec(&self) {
        let num = self.count.get();
        self.count.set(num - 1);
        if self.have_waiter.get() {
            match self.waker.borrow_mut().pop_front() {
                Some(waker) => waker.wake(),
                None => self.have_waiter.set(false),
            }
        }
    }

    fn available(&self, cx: &mut Context<'_>) -> bool {
        if self.count.get() < self.capacity {
            true
        } else {
            self.waker.borrow_mut().push_back(cx.waker().clone());
            self.have_waiter.set(true);
            false
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::future::Future;
    use std::pin::Pin;
    use std::task::Poll;
    use std::time::{Duration, Instant};

    use actix_rt::time::{delay_for, Delay};

    struct GetCounter<'a>(&'a Counter, Delay);

    impl<'a> GetCounter<'a> {
        fn new(counter: &'a Counter, dur: Duration) -> Self {
            Self(counter, delay_for(dur))
        }
    }

    impl Future for GetCounter<'_> {
        type Output = Option<CounterGuard>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            if Pin::new(&mut this.1).poll(cx).is_ready() {
                return Poll::Ready(None);
            }

            if this.0.available(cx) {
                Poll::Ready(Some(this.0.get()))
            } else {
                Poll::Pending
            }
        }
    }

    #[actix_rt::test]
    async fn test_get() {
        let counter = Counter::new(2);

        let a = GetCounter::new(&counter, Duration::from_millis(1)).await;
        let b = GetCounter::new(&counter, Duration::from_millis(1)).await;
        let c = GetCounter::new(&counter, Duration::from_millis(1)).await;

        assert!(a.is_some());
        assert!(b.is_some());
        assert!(c.is_none());
    }

    #[actix_rt::test]
    async fn test_delay() {
        let counter = Counter::new(1);
        let a = GetCounter::new(&counter, Duration::from_millis(1)).await;
        let now = Instant::now();
        let delay = Duration::from_millis(150);
        actix_rt::spawn(async move {
            delay_for(delay).await;
            drop(a);
        });
        let b = GetCounter::new(&counter, delay + Duration::from_millis(100)).await;
        assert!(b.is_some());
        assert!(now.elapsed() > delay);
    }

    #[actix_rt::test]
    async fn test_batch() {
        let counter = Counter::new(100);

        let mut taken = Vec::new();
        for i in 0..101 {
            let g = GetCounter::new(&counter, Duration::from_millis(10)).await;
            if g.is_some() {
                taken.push(g);
            } else {
                assert_eq!(i, 100);
                break;
            }
        }

        let (tx, rx) = crate::oneshot::channel();

        actix_rt::spawn(async move {
            let fut = (0..100)
                .map(|_| async {
                    let g = GetCounter::new(&counter, Duration::from_secs(10)).await;
                    assert!(g.is_some());
                })
                .collect::<Vec<_>>();

            futures_util::future::join_all(fut).await;

            let _ = tx.send(());
        });

        delay_for(Duration::from_secs(1)).await;
        drop(taken);
        let _ = rx.await;
    }
}
