//! Only model metadata is imported from Hub; connection settings and MCP stay local.
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

const MODELS_URL: &str = "https://hub.coreinfra.ai/claude-desktop/config";
// Hub's stable Claude Desktop alias for DeepSeek V4.1 Flash.
const DEEPSEEK_FLASH: &str = "claude-coreinfra-cn-1";

#[derive(Deserialize, Serialize)]
#[serde(transparent)]
pub struct Models(Vec<Model>);

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    behaves_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_effort: Option<Effort>,
    #[serde(rename = "supports1m", skip_serializing_if = "Option::is_none")]
    supports_1m: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Models {
    pub async fn fetch(token: &str) -> Result<Self> {
        let client = crate::hub::client()
            .map_err(|_| anyhow!("Не удалось создать клиент каталога Claude Desktop"))?;
        Self::fetch_from(&client, MODELS_URL, token).await
    }

    async fn fetch_from(client: &reqwest::Client, url: &str, token: &str) -> Result<Self> {
        let response = client
            .get(url)
            .header("x-api-key", token)
            .send()
            .await
            .map_err(|_| {
                anyhow!("Не удалось загрузить модели Claude Desktop. Попробуйте ещё раз.")
            })?;
        ensure!(
            response.status().is_success(),
            "Не удалось загрузить модели Claude Desktop (HTTP {})",
            response.status()
        );
        let bytes = response
            .bytes()
            .await
            .map_err(|_| anyhow!("Не удалось прочитать каталог Claude Desktop"))?;
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Catalog {
            #[serde(rename = "inferenceModels")]
            models: Models,
        }
        let catalog: Catalog = serde_json::from_slice(bytes)
            .map_err(|_| anyhow!("Hub вернул некорректный каталог Claude Desktop"))?;
        ensure!(
            catalog.models.valid(),
            "Hub вернул пустой или некорректный каталог Claude Desktop"
        );
        Ok(catalog.models)
    }

    fn valid(&self) -> bool {
        let mut ids = HashSet::new();
        !self.0.is_empty()
            && self
                .0
                .iter()
                .all(|model| !model.name.trim().is_empty() && ids.insert(&model.name))
    }

    pub(super) fn from_value(value: &Value) -> Option<Self> {
        serde_json::from_value::<Self>(value.clone())
            .ok()
            .filter(Self::valid)
    }

    pub(super) fn default_model_index(&self) -> usize {
        self.0
            .iter()
            .position(|model| model.name == DEEPSEEK_FLASH)
            .unwrap_or(0)
    }

    pub(super) fn value(&self) -> Result<Value> {
        ensure!(self.valid(), "Каталог моделей Claude Desktop не загружен");
        // Desktop uses the first entry as its default. Keep Hub order for the rest.
        let mut models: Vec<_> = self.0.iter().collect();
        models[..=self.default_model_index()].rotate_right(1);
        Ok(serde_json::to_value(models)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn imports_only_model_metadata_and_rejects_unusable_catalogs() -> Result<()> {
        let models = Models::parse(&serde_json::to_vec(&json!({
            "inferenceModels": [{"name":"claude-coreinfra-cn-3", "labelOverride":"GLM", "behavesAs":"claude-sonnet-4-6", "maxEffort":"max", "command":"ignored"}],
            "inferenceGatewayApiKey":"not-the-selected-key", "managedMcpServers":[{"command":"ignored"}]
        }))?)?;
        assert_eq!(
            models.value()?,
            json!([{"name":"claude-coreinfra-cn-3", "labelOverride":"GLM", "behavesAs":"claude-sonnet-4-6", "maxEffort":"max"}])
        );
        for invalid in [
            json!([]),
            json!(["stale-model"]),
            json!([{"name":""}]),
            json!([{"name":"a"},{"name":"a"}]),
            json!([{"name":"a","maxEffort":"unknown"}]),
        ] {
            assert!(
                Models::parse(&serde_json::to_vec(&json!({"inferenceModels":invalid}))?).is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn fetch_sends_the_agent_key_and_rejects_http_errors_and_redirects() -> Result<()> {
        let _ = crate::hub::install_crypto_provider();
        let client = crate::hub::client().map_err(|_| anyhow!("test client"))?;
        for status in [200, 401, 503, 302] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = format!("http://{}/claude-desktop/config", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).await?;
                    request.push(byte[0]);
                }
                let body = r#"{"inferenceModels":[{"name":"claude-sonnet-5"}]}"#;
                stream.write_all(format!("HTTP/1.1 {status} Test\r\nLocation: http://127.0.0.1:9/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
                Ok::<_, anyhow::Error>(String::from_utf8(request)?)
            });
            let result = Models::fetch_from(&client, &url, "fixture-key").await;
            let request = server.await??;
            assert!(request.starts_with("GET /claude-desktop/config "));
            assert!(request.contains("x-api-key: fixture-key\r\n"));
            if status == 200 {
                assert_eq!(result?.value()?, json!([{"name":"claude-sonnet-5"}]));
            } else {
                let error = result.err().expect("HTTP failure").to_string();
                assert!(error.contains(&status.to_string()));
                assert!(!error.contains("fixture-key"));
            }
        }
        Ok(())
    }
}
