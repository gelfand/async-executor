use std::sync::atomic::{fence, AtomicBool, Ordering};

pub struct SpinLock {
    flag: AtomicBool,
}

impl SpinLock {
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    pub fn lock(&self) {
        // Wait until the old value is `false`.
        while self
            .flag
            .compare_exchange_weak(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {}
        // This fence synchronizes-with store in `unlock`.
        fence(Ordering::Acquire);
    }

    #[inline(always)]
    pub fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
    }
}
