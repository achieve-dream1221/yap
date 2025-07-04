use crossbeam::channel::SendError;

pub type HandleResult<T> = Result<T, WorkerMissing>;

#[derive(Debug, thiserror::Error)]
#[error("worker listener has been dropped")]
pub struct WorkerMissing;

impl<T> From<SendError<T>> for WorkerMissing {
    fn from(_: SendError<T>) -> Self {
        Self
    }
}
