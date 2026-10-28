use alloc::sync::Arc;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hyper::header::HeaderValue;
use std::io;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio_rustls::{TlsConnector, client::TlsStream};

pub const HOST: &str = "proxy.hub.coreinfra.ai";
pub const PORT: u16 = 443;
pub const USERNAME: &str = "coreinfra";

// No Debug: the authorization header contains the saved token.
pub(crate) struct Upstream {
    connector: TlsConnector,
    pub(crate) authorization: HeaderValue,
}

impl Upstream {
    pub(crate) fn new(tls: ClientConfig, authorization: HeaderValue) -> Self {
        Self {
            connector: TlsConnector::from(Arc::new(tls)),
            authorization,
        }
    }

    pub(crate) async fn connect(&self) -> io::Result<TlsStream<TcpStream>> {
        // Deliberately bypass HTTP_PROXY/HTTPS_PROXY: selected destinations must
        // use this upstream, never an environment-provided proxy or direct fallback.
        let stream = crate::connect((HOST, PORT)).await?;
        let server_name = ServerName::try_from(HOST).map_err(io::Error::other)?;
        self.connector.connect(server_name, stream).await
    }
}

pub(crate) fn authorization(token: &str) -> io::Result<HeaderValue> {
    let value = format!("Basic {}", STANDARD.encode(format!("{USERNAME}:{token}")));
    let mut header = HeaderValue::from_str(&value).map_err(io::Error::other)?;
    header.set_sensitive(true);
    Ok(header)
}

pub(crate) fn tls_config() -> io::Result<ClientConfig> {
    let certificates = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (accepted, _) = roots.add_parsable_certificates(certificates.certs);
    if accepted == 0 {
        return Err(io::Error::other("no usable system CA certificates found"));
    }

    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}
