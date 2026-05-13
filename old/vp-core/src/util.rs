pub mod mutex_ext {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    pub trait MutexExt<T> {
        fn safe_lock(&'_ self) -> MutexGuard<'_, T>;
    }

    impl<T> MutexExt<T> for Mutex<T> {
        fn safe_lock(&self) -> MutexGuard<'_, T> {
            self.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }
}