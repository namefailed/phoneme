//! Connection to phoneme-daemon — wraps phoneme-ipc client.

use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::sync::Arc;
use tokio::sync::Mutex;

#[allow(dead_code)] // request() lands in Task 3 (Tauri commands)
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Mutex<NamedPipeTransport>>,
    pipe_name: String,
    pub config: Arc<Config>,
}

#[allow(dead_code)]
impl Bridge {
    pub async fn connect(config: Config) -> anyhow::Result<Self> {
        let pipe_name = config.daemon.pipe_name.clone();
        let transport = NamedPipeTransport::connect(&pipe_name).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(transport)),
            pipe_name,
            config: Arc::new(config),
        })
    }

    pub async fn reconnect(&self) -> anyhow::Result<()> {
        let new_transport = NamedPipeTransport::connect(&self.pipe_name).await?;
        let mut guard = self.inner.lock().await;
        *guard = new_transport;
        Ok(())
    }

    pub async fn request(&self, req: Request) -> anyhow::Result<Response> {
        let mut guard = self.inner.lock().await;
        match guard.request(req.clone()).await {
            Ok(r) => Ok(r),
            Err(_) => {
                drop(guard);
                self.reconnect().await?;
                let mut guard = self.inner.lock().await;
                Ok(guard.request(req).await?)
            }
        }
    }
}
