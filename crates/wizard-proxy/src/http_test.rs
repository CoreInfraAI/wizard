use crate::{Proxy, serve, upstream};
use alloc::sync::Arc;
use core::{net::SocketAddr, time::Duration};
use hyper::http::uri::Authority;
use socket2::SockRef;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

// No real token, system certificates, or external service is needed. Direct
// traffic must work independently of the authenticated upstream.
async fn start_proxy() -> io::Result<(SocketAddr, Proxy)> {
    let tls = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_root_certificates(RootCertStore::empty())
    .with_no_client_auth();
    let upstream = Arc::new(upstream::Upstream::new(
        tls,
        upstream::authorization("test-token")?,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown, receiver) = oneshot::channel();
    let (finished, stopped) = watch::channel(());
    let task = tokio::spawn(async move {
        let _finished = finished;
        serve(listener, address, upstream, receiver).await;
    });
    Ok((
        address,
        Proxy {
            shutdown,
            task,
            stopped,
        },
    ))
}

async fn read_head(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<String> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 8192, "HTTP head too large");
        bytes.push(stream.read_u8().await?);
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}

#[tokio::test]
async fn direct_http_rewrites_target_and_strips_only_proxy_credentials() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let origin = TcpListener::bind("127.0.0.1:0").await?;
        let target = origin.local_addr()?;
        let (address, proxy) = start_proxy().await?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await?;
            let head = read_head(&mut stream).await?.to_ascii_lowercase();
            assert!(head.starts_with("post /path?query=1 http/1.1\r\n"));
            assert!(head.contains(&format!("\r\nhost: {target}\r\n")));
            assert!(head.contains("\r\nauthorization: bearer origin-only\r\n"));
            assert!(!head.contains("proxy-authorization"));
            assert!(!head.contains("proxy-connection"));
            assert!(!head.contains("x-remove-me"));
            assert!(!head.contains("test-token"));
            let mut body = [0; 4];
            stream.read_exact(&mut body).await?;
            assert_eq!(&body, b"ping");
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close, x-remove-response\r\nX-Remove-Response: yes\r\nProxy-Authenticate: Basic\r\n\r\npong").await?;
            io::Result::Ok(())
        });
        let mut client = TcpStream::connect(address).await?;
        let request = format!("POST http://{target}/path?query=1 HTTP/1.1\r\nHost: wrong.example\r\nProxy-Authorization: Basic client-only\r\nAuthorization: Bearer origin-only\r\nConnection: keep-alive, x-remove-me\r\nProxy-Connection: keep-alive\r\nX-Remove-Me: yes\r\nContent-Length: 4\r\n\r\nping");
        client.write_all(request.as_bytes()).await?;
        let head = read_head(&mut client).await?.to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 200 "));
        assert!(!head.contains("proxy-authenticate"));
        assert!(!head.contains("x-remove-response"));
        let mut body = Vec::new();
        client.read_to_end(&mut body).await?;
        assert_eq!(body, b"pong");
        server.await.map_err(io::Error::other)??;
        proxy.stop().await
    }).await.expect("direct HTTP timed out")
}

#[tokio::test]
async fn listed_domains_reject_http_on_every_port() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let (address, proxy) = start_proxy().await?;
        for (target, status, message) in [
            ("API.OpenAI.COM:00443", 405, "Method Not Allowed"),
            ("computer.cluster.cursorvm.com:443", 405, "Method Not Allowed"),
            ("api.openai.com", 403, "Forbidden"),
            ("API.OpenAI.COM:00080", 403, "Forbidden"),
            ("cluster.cursorvm.com:8443", 403, "Forbidden"),
            ("computer.cluster.cursorvm.com:65535", 403, "Forbidden"),
        ] {
            for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE", "CUSTOM"] {
                let mut client = TcpStream::connect(address).await?;
                // Deliberately leave the body unsent: rejection must not wait for it.
                // A different Host must not bypass the absolute-form URI's policy.
                let request = format!("{method} http://{target}/path?query=1 HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\n\r\n");
                client.write_all(request.as_bytes()).await?;
                let head = read_head(&mut client).await?.to_ascii_lowercase();
                assert!(head.starts_with(&format!("http/1.1 {status} ")), "{method} {target}");
                if status == 405 {
                    assert!(head.contains("\r\nallow: connect\r\n"));
                } else {
                    assert!(!head.contains("\r\nallow:"));
                }
                assert!(head.contains("\r\nconnection: close\r\n"));
                let mut body = Vec::new();
                client.read_to_end(&mut body).await?;
                if method == "HEAD" {
                    assert!(body.is_empty());
                } else {
                    assert_eq!(body, message.as_bytes());
                }
            }
        }
        proxy.stop().await
    }).await.expect("method rejection timed out")
}

#[tokio::test]
async fn listed_domains_reject_connect_on_unlisted_ports() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let (address, proxy) = start_proxy().await?;
        for host in [
            "API.OpenAI.COM",
            "cluster.cursorvm.com",
            "computer.cluster.cursorvm.com",
        ] {
            for port in [80, 8443, 65535] {
                let mut client = TcpStream::connect(address).await?;
                let request =
                    format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
                client.write_all(request.as_bytes()).await?;
                let head = read_head(&mut client).await?.to_ascii_lowercase();
                assert!(head.starts_with("http/1.1 403 "), "{host}:{port}");
                assert!(!head.contains("\r\nallow:"));
                let mut body = Vec::new();
                client.read_to_end(&mut body).await?;
                assert_eq!(body, b"Forbidden");
            }
        }
        proxy.stop().await
    })
    .await
    .expect("blocked CONNECT timed out")
}

#[tokio::test]
async fn direct_connect_preserves_buffered_bytes_and_half_close() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let origin = TcpListener::bind("127.0.0.1:0").await?;
        let target = origin.local_addr()?;
        let (address, proxy) = start_proxy().await?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await?;
            let mut body = [0; 4];
            stream.read_exact(&mut body).await?;
            assert_eq!(&body, b"ping");
            stream.write_all(b"pong").await?;
            assert_eq!(stream.read(&mut body).await?, 0);
            stream.write_all(b"done").await?;
            io::Result::Ok(())
        });
        let mut client = TcpStream::connect(address).await?;
        client.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic client-only\r\n\r\nping").as_bytes()).await?;
        assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 200 "));
        let mut body = [0; 4];
        client.read_exact(&mut body).await?;
        assert_eq!(&body, b"pong");
        client.shutdown().await?;
        let mut tail = Vec::new();
        client.read_to_end(&mut tail).await?;
        assert_eq!(tail, b"done");
        server.await.map_err(io::Error::other)??;
        proxy.stop().await
    }).await.expect("direct CONNECT timed out")
}

#[tokio::test]
async fn half_closed_tunnel_to_silent_remote_closes_after_timeout() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let origin = TcpListener::bind("127.0.0.1:0").await?;
        let target = origin.local_addr()?;
        let (address, proxy) = start_proxy().await?;
        let mut client = TcpStream::connect(address).await?;
        client
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await?;
        // The remote stays open and silent after the client's FIN.
        let (mut remote, _) = origin.accept().await?;
        assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 200 "));
        client.shutdown().await?;
        let mut byte = [0];
        assert_eq!(remote.read(&mut byte).await?, 0);
        assert_eq!(client.read(&mut byte).await?, 0);
        proxy.stop().await
    })
    .await
    .expect("half-closed tunnel stayed open")
}

#[tokio::test]
async fn stopping_proxy_closes_both_direct_tunnel_sockets() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let origin = TcpListener::bind("127.0.0.1:0").await?;
        let target = origin.local_addr()?;
        let (address, proxy) = start_proxy().await?;
        let mut client = TcpStream::connect(address).await?;
        client
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await?;
        let (mut remote, _) = origin.accept().await?;
        assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 200 "));
        proxy.stop().await?;
        let mut byte = [0];
        assert_eq!(client.read(&mut byte).await?, 0);
        assert_eq!(remote.read(&mut byte).await?, 0);
        io::Result::Ok(())
    })
    .await
    .expect("proxy shutdown timed out")
}

#[tokio::test]
async fn refused_direct_connection_returns_bad_gateway() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let origin = TcpListener::bind("127.0.0.1:0").await?;
        let target = origin.local_addr()?;
        drop(origin);
        let (address, proxy) = start_proxy().await?;
        for request in [
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"),
            format!("GET http://{target}/ HTTP/1.1\r\nHost: {target}\r\n\r\n"),
        ] {
            let mut client = TcpStream::connect(address).await?;
            client.write_all(request.as_bytes()).await?;
            assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 502 "));
        }
        proxy.stop().await
    })
    .await
    .expect("connection failure timed out")
}

#[tokio::test]
async fn malformed_destination_is_rejected_before_dialing() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let (address, proxy) = start_proxy().await?;
        for request in [
            "CONNECT example.com HTTP/1.1\r\nHost: example.com\r\n\r\n",
            "CONNECT 127.0.0.1:0 HTTP/1.1\r\nHost: 127.0.0.1:0\r\n\r\n",
            "CONNECT user@example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            "CONNECT api.openai.com.:443 HTTP/1.1\r\nHost: api.openai.com.:443\r\n\r\n",
            "CONNECT api_openai.com:443 HTTP/1.1\r\nHost: api_openai.com:443\r\n\r\n",
            "CONNECT 127.1:443 HTTP/1.1\r\nHost: 127.1:443\r\n\r\n",
            "GET http://api.openai.com:65536/ HTTP/1.1\r\nHost: api.openai.com\r\n\r\n",
            "GET http://api.openai.com./ HTTP/1.1\r\nHost: api.openai.com.\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nContent-Length: 1\r\n\r\nx",
        ] {
            let mut client = TcpStream::connect(address).await?;
            client.write_all(request.as_bytes()).await?;
            assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 400 "));
        }
        proxy.stop().await
    }).await.expect("invalid destination timed out")
}

#[tokio::test]
async fn direct_connections_use_tcp_keepalive() -> io::Result<()> {
    let origin = TcpListener::bind("127.0.0.1:0").await?;
    let authority =
        Authority::try_from(origin.local_addr()?.to_string()).map_err(io::Error::other)?;
    let stream = super::connect_direct(&authority, 80).await?;
    assert!(SockRef::from(&stream).keepalive()?);
    Ok(())
}
