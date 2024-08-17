use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug)]
#[must_use = "released on drop"]
pub(crate) struct SemaphoreGuard(Arc<Semaphore>);

impl Drop for SemaphoreGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Simple semaphore implementation based on mutex + condvar. Used to limit parallelism when running
/// cachegrind-instrumented executables.
#[derive(Debug)]
pub(crate) struct Semaphore {
    permits: Mutex<usize>,
    cvar: Condvar,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Self {
            permits: Mutex::new(permits),
            cvar: Condvar::new(),
        }
    }

    pub fn acquire_owned(self: &Arc<Self>) -> SemaphoreGuard {
        let mut guard = self
            .cvar
            .wait_while(self.permits.lock().unwrap(), |permits| *permits == 0)
            .unwrap();
        *guard -= 1;
        drop(guard);

        SemaphoreGuard(self.clone())
    }

    fn release(&self) {
        *self.permits.lock().unwrap() += 1;
        self.cvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn using_semaphore() {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let semaphore = Arc::new(Semaphore::new(4));
        let threads: Vec<_> = (0..100)
            .map(|_| {
                let semaphore = semaphore.clone();
                thread::spawn(move || {
                    let _permit = semaphore.acquire_owned();
                    let old_value = COUNTER.fetch_add(1, Ordering::SeqCst);
                    assert!(old_value < 4, "{old_value}");
                    thread::sleep(Duration::from_millis(10));
                    COUNTER.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        for thread in threads {
            thread.join().unwrap();
        }
    }
}
