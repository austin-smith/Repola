use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct OperationToken(Arc<AtomicBool>);

impl OperationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

thread_local! {
    static CURRENT_OPERATION: RefCell<Option<OperationToken>> = const { RefCell::new(None) };
}

pub(crate) fn with_operation<T>(token: OperationToken, execute: impl FnOnce() -> T) -> T {
    CURRENT_OPERATION.with(|current| {
        struct Restore<'a> {
            current: &'a RefCell<Option<OperationToken>>,
            previous: Option<OperationToken>,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.current.replace(self.previous.take());
            }
        }
        let restore = Restore {
            current,
            previous: current.replace(Some(token)),
        };
        let result = execute();
        drop(restore);
        result
    })
}

pub(crate) fn current_operation() -> OperationToken {
    CURRENT_OPERATION
        .with(|current| current.borrow().clone())
        .unwrap_or_default()
}

pub(crate) fn is_cancelled() -> bool {
    CURRENT_OPERATION.with(|current| {
        current
            .borrow()
            .as_ref()
            .is_some_and(OperationToken::is_cancelled)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_scope_restores_the_previous_token() {
        let outer = OperationToken::new();
        with_operation(outer.clone(), || {
            assert!(!is_cancelled());
            outer.cancel();
            assert!(is_cancelled());
            with_operation(OperationToken::new(), || assert!(!is_cancelled()));
            assert!(is_cancelled());
        });
        assert!(!is_cancelled());
    }

    #[test]
    fn operation_scope_is_restored_during_unwind() {
        let _ = std::panic::catch_unwind(|| {
            with_operation(OperationToken::new(), || panic!("test panic"));
        });
        assert!(!is_cancelled());
    }
}
