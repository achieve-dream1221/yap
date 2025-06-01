pub type YapResult<T> = Result<T, YapError>;

#[derive(Debug, thiserror::Error)]
pub enum YapError {
    #[error("No Serial Worker")]
    NoSerialWorker,
    #[error("No Carousel Worker")]
    NoCarouselWorker,
    #[error("No Logging Worker")]
    NoLoggingWorker,
}
