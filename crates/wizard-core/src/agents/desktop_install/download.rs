//! Resumable archive downloads. The archive and the release it belongs to stay in Wizard's cache
//! across pauses and restarts, until installation, another release or failed verification
//! discards them.

use super::{
    ArchiveVerifier, DOWNLOAD_NAME, DesktopApp, DownloadedRelease, Integrity, MAX_DOWNLOAD,
    Release, client,
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::{fmt, time::Duration};
use reqwest::{Client, Response, StatusCode, header};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt as _;
use std::{
    fs,
    io::{self, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

const RECORD_NAME: &str = "download.json";

/// Delays before reconnecting after consecutive attempts that received nothing: about two
/// minutes in all. The download then fails, and continuing it later keeps what was saved.
const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
    Duration::from_mins(1),
];

/// Download progress, counting bytes saved before a pause or restart. `total` is 0 until known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
    /// The connection failed; the download continues from `received` after a delay.
    pub reconnecting: bool,
}

/// Part of a release's archive that an earlier download saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SavedDownload {
    pub received: u64,
    pub total: u64,
}

impl SavedDownload {
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.received == self.total
    }
}

/// What an earlier download saved toward `release`. Anything saved toward another release, or
/// toward none because `app` is up to date, is deleted.
pub fn saved_download(app: DesktopApp, release: Option<&Release>) -> Result<Option<SavedDownload>> {
    Ok(Slot::new(app)?
        .saved(release)
        .filter(|(_, received)| *received > 0)
        .map(|(record, received)| SavedDownload {
            received,
            total: record.size,
        }))
}

/// Downloads and verifies the release archive, continuing a saved download of it. What was
/// received stays saved when the download fails or is dropped; failed verification discards it.
pub async fn download_release(
    release: Release,
    token: &str,
    progress: impl FnMut(DownloadProgress),
) -> Result<DownloadedRelease> {
    let slot = Slot::new(release.target.app)?;
    download(slot, release, &client(token)?, &RETRY_DELAYS, progress).await
}

async fn download(
    slot: Slot,
    release: Release,
    client: &Client,
    delays: &[Duration],
    mut progress: impl FnMut(DownloadProgress),
) -> Result<DownloadedRelease> {
    let name = release.target.app.name();
    let product = release.target.app.product();
    let saved = slot.saved(Some(&release));
    slot.create()?;
    let archive = slot.archive();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&archive)
        .context("Не удалось открыть архив")?;

    let mut verifier = ArchiveVerifier::new(&release.integrity, name)?;
    let (record, received) = saved.map_or((None, 0), |(record, received)| (Some(record), received));
    if let Some(record) = &record
        && received > 0
    {
        log::info!(
            "{product} download continues from {received} of {} bytes",
            record.size
        );
        progress(DownloadProgress {
            received,
            total: record.size,
            reconnecting: false,
        });
        verifier = rehash(archive, received, verifier).await?;
    }
    // Drops anything after the saved part, such as an old archive that could not be deleted.
    file.set_len(received)
        .and_then(|()| file.seek(SeekFrom::Start(received)))
        .context("Не удалось открыть архив")?;

    let mut transfer = Transfer {
        slot: &slot,
        release: &release,
        file,
        received,
        record,
        verifier,
    };
    let mut failures = 0;
    while !transfer.complete() {
        let before = transfer.received;
        match transfer.attempt(client, &mut progress).await {
            Ok(()) => {}
            Err(error) if error.is::<Interrupted>() => {
                if transfer.received > before {
                    failures = 0;
                }
                let Some(delay) = delays.get(failures) else {
                    log::warn!("{product} download interrupted; reconnection attempts exhausted");
                    return Err(error);
                };
                failures += 1;
                log::warn!(
                    "{product} download interrupted; reconnecting in {} s (attempt {failures} of {})",
                    delay.as_secs(),
                    delays.len()
                );
                progress(DownloadProgress {
                    received: transfer.received,
                    total: transfer.total(),
                    reconnecting: true,
                });
                tokio::time::sleep(*delay).await;
            }
            Err(error) => return Err(error),
        }
    }

    let Transfer { file, verifier, .. } = transfer;
    file.sync_all().context("Не удалось записать архив")?;
    // Windows cannot delete an open file.
    drop(file);
    match verifier.finish(&release.integrity, name) {
        Ok(sha256) => Ok(DownloadedRelease {
            directory: slot.directory,
            release,
            sha256,
        }),
        Err(error) => {
            log::warn!("{product} archive failed verification and was deleted");
            discard(&slot.directory);
            Err(error)
        }
    }
}

/// Deletes a saved download: its archive, record and leftovers of installation attempts.
pub(super) fn discard(directory: &Path) {
    // Without its record the archive is never used again, even if it cannot be deleted.
    fs::remove_file(directory.join(RECORD_NAME)).ok();
    match fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("Saved download could not be deleted: {:?}", error.kind()),
    }
}

/// A failure that reconnecting can overcome: a lost connection, a stall or a busy server.
#[derive(Debug)]
struct Interrupted(String);

impl fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for Interrupted {}

fn interrupted(message: String) -> anyhow::Error {
    anyhow::Error::new(Interrupted(message))
}

impl Integrity {
    /// Identifies the archive contents that the feed promises.
    fn key(&self) -> String {
        use base64::Engine as _;

        match self {
            Self::Sha256(sha256) => format!("sha256:{sha256}"),
            Self::Ed25519 { signature, .. } => format!(
                "ed25519:{}",
                base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            ),
            Self::PackageSignature => "package-signature".to_owned(),
        }
    }
}

/// The release that a saved archive belongs to.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Record {
    url: String,
    integrity: String,
    /// Archive size, from the feed or the server.
    size: u64,
    /// Strong validator of the archive on the server: a download continues only while it holds.
    etag: Option<String>,
}

impl Record {
    fn matches(&self, release: &Release) -> bool {
        self.url == release.url.as_str()
            && self.integrity == release.integrity.key()
            && release.size.is_none_or(|size| size == self.size)
    }
}

/// Wizard's saved download of one application.
struct Slot {
    directory: PathBuf,
}

impl Slot {
    fn new(app: DesktopApp) -> Result<Self> {
        let name = match app {
            DesktopApp::Claude => "claude",
            DesktopApp::ChatGpt => "chatgpt",
        };
        let cache =
            wizard_settings::paths::cache_dir().context("Не удалось найти каталог загрузок")?;
        Ok(Self {
            directory: cache.join("downloads").join(name),
        })
    }

    fn archive(&self) -> PathBuf {
        self.directory.join(DOWNLOAD_NAME)
    }

    fn create(&self) -> Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(&self.directory)
            .context("Не удалось создать каталог загрузки")
    }

    /// The record and saved length of `release`'s archive. Anything else is deleted.
    fn saved(&self, release: Option<&Release>) -> Option<(Record, u64)> {
        let saved = fs::read(self.directory.join(RECORD_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
            .filter(|record| release.is_some_and(|release| record.matches(release)))
            .and_then(|record| {
                let length = fs::metadata(self.archive()).map_or(0, |metadata| metadata.len());
                (length <= record.size).then_some((record, length))
            });
        if saved.is_none() {
            discard(&self.directory);
        }
        saved
    }

    fn write_record(&self, record: &Record) -> Result<()> {
        let temporary = self.directory.join(format!("{RECORD_NAME}.tmp"));
        fs::write(&temporary, serde_json::to_vec(record)?)
            .and_then(|()| fs::rename(&temporary, self.directory.join(RECORD_NAME)))
            .context("Не удалось сохранить сведения о загрузке")
    }
}

/// An archive being received into its slot.
struct Transfer<'a> {
    slot: &'a Slot,
    release: &'a Release,
    file: fs::File,
    /// Bytes in `file`, all fed to `verifier`.
    received: u64,
    /// Saved once the server confirms the archive size.
    record: Option<Record>,
    verifier: ArchiveVerifier,
}

impl Transfer<'_> {
    fn total(&self) -> u64 {
        self.record
            .as_ref()
            .map(|record| record.size)
            .or(self.release.size)
            .unwrap_or(0)
    }

    fn complete(&self) -> bool {
        self.record
            .as_ref()
            .is_some_and(|record| self.received == record.size)
    }

    /// Drops the received part: the server no longer serves the archive it belongs to.
    fn restart(&mut self) -> Result<()> {
        self.file
            .set_len(0)
            .and_then(|()| self.file.seek(SeekFrom::Start(0)))
            .context("Не удалось записать архив")?;
        self.received = 0;
        self.verifier =
            ArchiveVerifier::new(&self.release.integrity, self.release.target.app.name())?;
        Ok(())
    }

    /// Requests the rest of the archive. Returns once it is complete; an `Interrupted` error
    /// leaves what was received for the next attempt.
    async fn attempt(
        &mut self,
        client: &Client,
        progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<()> {
        let name = self.release.target.app.name();
        let mut request = client.get(self.release.url.clone());
        if self.received > 0 {
            request = request.header(header::RANGE, format!("bytes={}-", self.received));
            if let Some(etag) = self.record.as_ref().and_then(|record| record.etag.as_ref()) {
                // The server sends the whole archive instead if it has changed.
                request = request.header(header::IF_RANGE, etag);
            }
        }
        let mut response = request.send().await.map_err(|error| {
            let message =
                format!("Не удалось скачать {name} через прокси; проверьте токен и соединение");
            if proxy_rejected_token(&error) {
                anyhow!(message)
            } else {
                interrupted(message)
            }
        })?;

        let status = response.status();
        match status {
            StatusCode::PARTIAL_CONTENT if self.received > 0 => self.accept_range(&response)?,
            StatusCode::OK => self.accept_whole(&response)?,
            StatusCode::RANGE_NOT_SATISFIABLE => {
                self.restart()?;
                return Err(interrupted(
                    "Сервер загрузки не продолжил загрузку архива".to_owned(),
                ));
            }
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS => {
                return Err(interrupted(format!(
                    "Сервер загрузки вернул HTTP {}",
                    status.as_u16()
                )));
            }
            _ if status.is_server_error() => {
                return Err(interrupted(format!(
                    "Сервер загрузки вернул HTTP {}",
                    status.as_u16()
                )));
            }
            _ => bail!("Сервер загрузки вернул HTTP {}", status.as_u16()),
        }

        let size = self.total();
        let mut published = self.received;
        progress(DownloadProgress {
            received: self.received,
            total: size,
            reconnecting: false,
        });
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| interrupted(format!("Загрузка {name} прервана")))?
        {
            let received = self.received + chunk.len() as u64;
            if received > size {
                self.restart()?;
                bail!("Размер архива превышает заявленный");
            }
            self.file
                .write_all(&chunk)
                .context("Не удалось записать архив")?;
            self.verifier.update(&chunk);
            self.received = received;
            if received - published >= 1024 * 1024 || received == size {
                progress(DownloadProgress {
                    received,
                    total: size,
                    reconnecting: false,
                });
                published = received;
            }
        }
        if self.received < size {
            return Err(interrupted(format!("Загрузка {name} прервана")));
        }
        Ok(())
    }

    /// Continues with a response that holds the rest of the saved archive.
    fn accept_range(&mut self, response: &Response) -> Result<()> {
        let Some(record) = &self.record else {
            bail!("Сервер загрузки вернул HTTP 206");
        };
        let expected = format!(
            "bytes {}-{}/{}",
            self.received,
            record.size - 1,
            record.size
        );
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|range| range.to_str().ok());
        if range != Some(expected.as_str()) {
            log::info!(
                "{} archive range does not match the saved part; the download starts over",
                self.release.target.app.product()
            );
            self.restart()?;
            return Err(interrupted(
                "Сервер загрузки вернул другую часть архива".to_owned(),
            ));
        }
        Ok(())
    }

    /// Starts over with a response that holds the whole archive.
    fn accept_whole(&mut self, response: &Response) -> Result<()> {
        if self.received > 0 {
            log::info!(
                "{} archive changed on the server; the download starts over",
                self.release.target.app.product()
            );
            self.restart()?;
        }
        let size = match self.release.size {
            Some(size) => size,
            None => response
                .content_length()
                .context("Сервер загрузки не сообщил размер архива")?,
        };
        ensure!(
            (1..=MAX_DOWNLOAD).contains(&size),
            "Недопустимый размер архива"
        );
        if let Some(length) = response.content_length() {
            ensure!(length == size, "Размер архива не совпадает с заявленным");
        }
        let record = Record {
            url: self.release.url.to_string(),
            integrity: self.release.integrity.key(),
            size,
            etag: strong_etag(response),
        };
        // Save the record first: a restart then finds the archive it describes.
        self.slot.write_record(&record)?;
        self.record = Some(record);
        Ok(())
    }
}

/// Whether the proxy refused the token, which reconnecting cannot fix. hyper-util's error type is
/// private, so only its message identifies the refusal.
fn proxy_rejected_token(error: &reqwest::Error) -> bool {
    core::iter::successors(
        Some(error as &(dyn core::error::Error + 'static)),
        |error| error.source(),
    )
    .any(|error| error.to_string() == "tunnel error: proxy authorization required")
}

/// The archive's `ETag`, if it can validate a range request.
fn strong_etag(response: &Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)?
        .to_str()
        .ok()
        .filter(|etag| !etag.starts_with("W/"))
        .map(ToOwned::to_owned)
}

/// Feeds the saved part of the archive to `verifier`, so the download continues after it.
async fn rehash(
    archive: PathBuf,
    length: u64,
    mut verifier: ArchiveVerifier,
) -> Result<ArchiveVerifier> {
    tokio::task::spawn_blocking(move || {
        let mut file = fs::File::open(&archive)?.take(length);
        let mut buffer = vec![0; 1024 * 1024];
        let mut read = 0;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            verifier.update(&buffer[..count]);
            read += count as u64;
        }
        ensure!(read == length, "Сохранённая часть архива изменилась");
        Ok(verifier)
    })
    .await
    .context("Не удалось проверить сохранённую часть архива")?
    .context("Не удалось прочитать сохранённую часть архива")
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "bounded test fixtures")]
mod tests {
    use super::*;
    use crate::agents::desktop_install::Target;
    use reqwest::Url;
    use semver::Version;
    use sha2::{Digest as _, Sha256};
    use std::{io::BufRead as _, net::TcpListener, sync::Mutex, thread};

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Request {
        range: Option<String>,
        if_range: Option<String>,
    }

    #[derive(Clone, Copy)]
    enum Plan {
        Serve,
        /// Serves the response but closes the connection after this many body bytes.
        CutAfter(usize),
        Status(u16),
    }

    struct Server {
        url: String,
        etag: &'static Mutex<String>,
        requests: &'static Mutex<Vec<Request>>,
    }

    impl Server {
        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    /// Serves `archive` with range requests, following one plan per request, then `Plan::Serve`.
    fn serve(archive: Vec<u8>, plans: Vec<Plan>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/download.zip", listener.local_addr().unwrap());
        // Each server lives until the test process exits.
        let etag: &'static _ = Box::leak(Box::new(Mutex::new("\"one\"".to_owned())));
        let requests: &'static _ = Box::leak(Box::new(Mutex::new(Vec::new())));
        let (current_etag, log) = (etag, requests);
        thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let mut stream = stream.unwrap();
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut request = Request {
                    range: None,
                    if_range: None,
                };
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(": ") {
                        match name.to_ascii_lowercase().as_str() {
                            "range" => request.range = Some(value.to_owned()),
                            "if-range" => request.if_range = Some(value.to_owned()),
                            _ => {}
                        }
                    }
                }
                log.lock().unwrap().push(request.clone());

                let etag = current_etag.lock().unwrap().clone();
                let plan = plans.get(index).copied().unwrap_or(Plan::Serve);
                let (status, range, body) = if let Plan::Status(status) = plan {
                    (status, String::new(), &[][..])
                } else {
                    let start = request
                        .range
                        .as_deref()
                        .and_then(|range| range.strip_prefix("bytes="))
                        .and_then(|range| range.strip_suffix('-'))
                        .filter(|_| request.if_range.as_ref().is_none_or(|tag| *tag == etag))
                        .map(|start| start.parse::<usize>().unwrap());
                    match start {
                        Some(start) => (
                            206,
                            format!(
                                "Content-Range: bytes {start}-{}/{}\r\n",
                                archive.len() - 1,
                                archive.len()
                            ),
                            &archive[start..],
                        ),
                        None => (200, String::new(), archive.as_slice()),
                    }
                };
                let sent = match plan {
                    Plan::CutAfter(limit) => &body[..limit.min(body.len())],
                    Plan::Serve | Plan::Status(_) => body,
                };
                // One write, as real servers do: hyper-util reads a proxy's status from the
                // first packet.
                let head = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n{range}\r\n",
                    body.len()
                );
                // The client may close early; the test then checks what it received.
                let _ = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(sent));
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        Server {
            url,
            etag,
            requests,
        }
    }

    fn archive() -> Vec<u8> {
        (0..3_000_000_u32)
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 24).to_le_bytes()[0])
            .collect()
    }

    fn release(url: &str, archive: &[u8]) -> Release {
        Release {
            version: Version::new(1, 0, 0),
            display_version: "1.0.0".to_owned(),
            target: Target::new(DesktopApp::Claude, None, None),
            url: Url::parse(url).unwrap(),
            integrity: Integrity::Sha256(format!("{:x}", Sha256::digest(archive))),
            size: Some(archive.len() as u64),
        }
    }

    fn run(
        directory: &Path,
        release: &Release,
        delays: &[Duration],
    ) -> (Result<DownloadedRelease>, Vec<DownloadProgress>) {
        run_through(None, directory, release, delays)
    }

    fn run_through(
        proxy: Option<&str>,
        directory: &Path,
        release: &Release,
        delays: &[Duration],
    ) -> (Result<DownloadedRelease>, Vec<DownloadProgress>) {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let mut client = Client::builder().no_proxy();
        if let Some(proxy) = proxy {
            client = client.proxy(reqwest::Proxy::all(proxy).unwrap());
        }
        let client = client.build().unwrap();
        let slot = Slot {
            directory: directory.to_owned(),
        };
        let mut events = Vec::new();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(download(slot, release.clone(), &client, delays, |event| {
                events.push(event);
            }));
        (result, events)
    }

    fn saved(directory: &Path, release: Option<&Release>) -> Option<(Record, u64)> {
        Slot {
            directory: directory.to_owned(),
        }
        .saved(release)
    }

    #[test]
    fn interrupted_download_continues_from_saved_part() {
        let archive = archive();
        let server = serve(archive.clone(), vec![Plan::CutAfter(1_000_000)]);
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("claude");

        let (result, _) = run(&directory, &release, &[]);
        let error = result.err().unwrap();
        assert!(error.is::<Interrupted>());
        assert_eq!(error.to_string(), "Загрузка Claude прервана");
        let (record, received) = saved(&directory, Some(&release)).unwrap();
        assert!((1..=1_000_000).contains(&received));
        assert_eq!(record.etag.as_deref(), Some("\"one\""));
        assert_eq!(record.size, archive.len() as u64);

        let (result, events) = run(&directory, &release, &[]);
        let downloaded = result.unwrap();
        assert_eq!(downloaded.sha256, format!("{:x}", Sha256::digest(&archive)));
        assert_eq!(fs::read(directory.join(DOWNLOAD_NAME)).unwrap(), archive);
        assert_eq!(
            events.first(),
            Some(&DownloadProgress {
                received,
                total: archive.len() as u64,
                reconnecting: false,
            })
        );
        assert_eq!(
            server.requests()[1],
            Request {
                range: Some(format!("bytes={received}-")),
                if_range: Some("\"one\"".to_owned()),
            }
        );
    }

    #[test]
    fn download_reconnects_until_attempts_without_progress_run_out() {
        let archive = archive();
        let server = serve(
            archive.clone(),
            vec![
                Plan::CutAfter(500_000),
                Plan::Status(503),
                Plan::CutAfter(0),
            ],
        );
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("claude");

        // Progress resets the count: the cut response is followed by two empty attempts.
        let (result, events) = run(&directory, &release, &[Duration::ZERO; 2]);
        let error = result.err().unwrap();
        assert_eq!(error.to_string(), "Загрузка Claude прервана");
        assert_eq!(server.requests().len(), 3);
        let reconnecting = events.iter().filter(|event| event.reconnecting).count();
        assert_eq!(reconnecting, 2);

        let (result, _) = run(&directory, &release, &[Duration::ZERO; 2]);
        result.unwrap();
        assert_eq!(fs::read(directory.join(DOWNLOAD_NAME)).unwrap(), archive);
        assert!(
            server.requests()[1..]
                .iter()
                .all(|request| request.range.is_some())
        );
    }

    #[test]
    fn client_errors_are_not_retried() {
        let archive = archive();
        let server = serve(archive.clone(), vec![Plan::Status(404)]);
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();

        let (result, _) = run(&root.path().join("claude"), &release, &[Duration::ZERO; 5]);
        let error = result.err().unwrap();
        assert!(!error.is::<Interrupted>());
        assert_eq!(error.to_string(), "Сервер загрузки вернул HTTP 404");
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn proxy_rejecting_the_token_is_not_retried() {
        let archive = archive();
        let proxy = serve(archive.clone(), vec![Plan::Status(407)]);
        let release = release("https://downloads.invalid/download.zip", &archive);
        let root = tempfile::tempdir().unwrap();

        let (result, _) = run_through(
            Some(proxy.url.trim_end_matches("/download.zip")),
            &root.path().join("claude"),
            &release,
            &[Duration::ZERO; 5],
        );
        let error = result.err().unwrap();
        assert!(!error.is::<Interrupted>());
        assert_eq!(
            error.to_string(),
            "Не удалось скачать Claude через прокси; проверьте токен и соединение"
        );
        assert_eq!(proxy.requests().len(), 1);
    }

    #[test]
    fn changed_server_archive_starts_over() {
        let archive = archive();
        let server = serve(archive.clone(), vec![Plan::CutAfter(1_000_000)]);
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("claude");

        assert!(run(&directory, &release, &[]).0.is_err());
        "\"two\"".clone_into(&mut server.etag.lock().unwrap());
        let (result, events) = run(&directory, &release, &[]);
        result.unwrap();
        assert_eq!(fs::read(directory.join(DOWNLOAD_NAME)).unwrap(), archive);
        assert_eq!(
            server.requests()[1].if_range.as_deref(),
            Some("\"one\""),
            "the saved part is requested only while unchanged"
        );
        assert!(events.contains(&DownloadProgress {
            received: 0,
            total: archive.len() as u64,
            reconnecting: false,
        }));
        let (record, _) = saved(&directory, Some(&release)).unwrap();
        assert_eq!(record.etag.as_deref(), Some("\"two\""));
    }

    #[test]
    fn complete_archive_is_verified_without_downloading_again() {
        let archive = archive();
        let server = serve(archive.clone(), Vec::new());
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("claude");

        run(&directory, &release, &[]).0.unwrap();
        run(&directory, &release, &[]).0.unwrap();
        assert_eq!(server.requests().len(), 1);

        let mut corrupt = archive;
        corrupt[10] ^= 1;
        fs::write(directory.join(DOWNLOAD_NAME), corrupt).unwrap();
        let error = run(&directory, &release, &[]).0.err().unwrap();
        assert_eq!(error.to_string(), "SHA-256 архива не совпадает");
        assert!(!directory.exists());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn saved_download_of_another_release_is_discarded() {
        let archive = archive();
        let server = serve(archive.clone(), vec![Plan::CutAfter(1_000_000); 2]);
        let release = release(&server.url, &archive);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("claude");
        assert!(run(&directory, &release, &[]).0.is_err());

        assert!(saved(&directory, Some(&release)).is_some());
        let mut newer = release.clone();
        newer.url = Url::parse(&format!("{}?version=2", server.url)).unwrap();
        assert!(saved(&directory, Some(&newer)).is_none());
        assert!(!directory.exists());

        assert!(run(&directory, &release, &[]).0.is_err());
        assert!(saved(&directory, None).is_none());
        assert!(!directory.exists());
    }
}
