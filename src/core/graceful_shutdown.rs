use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct GracefulShutdown<T = ()> {
    pub join_handle: JoinHandle<T>,
    pub cancellation_token: CancellationToken,
}

impl<T> GracefulShutdown<T> {
    pub fn new(join_handle: JoinHandle<T>, cancellation_token: CancellationToken) -> Self {
        Self {
            join_handle,
            cancellation_token,
        }
    }

    pub fn abort(self) {
        self.join_handle.abort();
    }

    pub async fn shutdown(self) -> anyhow::Result<T> {
        self.cancellation_token.cancel();
        let result = self.join_handle.await?;
        Ok(result)
    }
}
