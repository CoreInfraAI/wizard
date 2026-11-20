//! Video listener owned by the managed daemon, independent of GUI lifetime.
use super::{VideoHub, VideoKey, keys::VideoKeys, server};
use core::{future::Future, pin::Pin};
use tokio::{net::TcpListener, task::JoinHandle};
use wizard_proxy::VideoService;
use wizard_settings::VideoSettings;

#[derive(Default)]
pub struct Service {
    task: Option<JoinHandle<()>>,
    active: Option<(VideoSettings, VideoKeys)>,
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Service {
    async fn stop(&mut self) {
        self.active = None;
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }

    async fn apply(&mut self, grants: VideoSettings) -> Result<(), &'static str> {
        if !grants.has_grants() {
            self.stop().await;
            return Ok(());
        }
        let Ok(keys) = tokio::task::spawn_blocking(VideoKeys::load).await else {
            self.stop().await;
            return Err("Не удалось загрузить ключи для видео");
        };
        self.apply_routes(grants, keys, &super::ADDRESS.to_string())
            .await
    }

    async fn apply_routes(
        &mut self,
        grants: VideoSettings,
        keys: VideoKeys,
        address: &str,
    ) -> Result<(), &'static str> {
        if self.running()
            && self
                .active
                .as_ref()
                .is_some_and(|(g, k)| *g == grants && *k == keys)
        {
            return Ok(());
        }
        self.stop().await;
        let routes = [
            (&grants.codex, &keys.codex),
            (&grants.claude, &keys.claude),
            (&grants.claude_desktop, &keys.claude_desktop),
        ]
        .into_iter()
        .filter_map(|(grant, key)| {
            Some((grant.clone()?, key.clone().map(|token| VideoKey { token })))
        })
        .collect();
        if tokio_rustls::rustls::crypto::CryptoProvider::get_default().is_none() {
            crate::hub::install_crypto_provider()
                .map_err(|_| "Не удалось настроить TLS для видео")?;
        }
        let hub = VideoHub::new().map_err(|_| "Не удалось создать клиент видео")?;
        let listener = TcpListener::bind(address)
            .await
            .map_err(|_| "Не удалось открыть локальный порт видео 33789")?;
        let addr = listener
            .local_addr()
            .map_err(|_| "Не удалось определить адрес видео")?;
        self.task = Some(tokio::spawn(server::serve(listener, addr, hub, routes)));
        self.active = Some((grants, keys));
        Ok(())
    }
}

impl VideoService for Service {
    fn running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }
    fn reload(
        &mut self,
        grants: VideoSettings,
    ) -> Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + '_>> {
        Box::pin(self.apply(grants))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reload_revokes_only_removed_bearer_and_last_grant_releases_port() -> anyhow::Result<()>
    {
        let socket = TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?.to_string();
        drop(socket);
        let url = format!("http://{address}/mcp");
        let mut service = Service::default();
        let mut grants = VideoSettings {
            codex: Some("one".into()),
            claude: Some("two".into()),
            ..Default::default()
        };
        service
            .apply_routes(grants.clone(), VideoKeys::default(), &address)
            .await
            .map_err(anyhow::Error::msg)?;
        // Reload replaces the listener. Check grants on fresh connections, not pooled ones.
        let client = reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()?;
        let list = |bearer| {
            client
                .post(&url)
                .bearer_auth(bearer)
                .header("Accept", "application/json, text/event-stream")
                .header("Content-Type", "application/json")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
                .send()
        };
        assert!(list("one").await?.status().is_success());
        grants.codex = None;
        service
            .apply_routes(grants, VideoKeys::default(), &address)
            .await
            .map_err(anyhow::Error::msg)?;
        assert_eq!(
            list("one").await?.status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert!(list("two").await?.status().is_success());
        service
            .reload(VideoSettings::default())
            .await
            .map_err(anyhow::Error::msg)?;
        assert!(!service.running());
        TcpListener::bind(&address).await?;
        Ok(())
    }
}
