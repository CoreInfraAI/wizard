//! Error diagnostics without request data, credentials, or destinations.

use core::{error::Error, fmt::Write as _};
use std::io;

/// Describe typed failures and native OS errors, never arbitrary error messages.
/// Custom error strings can contain URLs, credentials, or certificate identities.
pub fn summary(error: &(dyn Error + 'static)) -> String {
    let mut description = String::new();
    let mut current = Some(error);
    for _ in 0..32 {
        let Some(error) = current else { break };
        if let Some(error) = error.downcast_ref::<io::Error>() {
            let _ = write!(description, " kind={:?}", error.kind());
            if let Some(code) = error.raw_os_error() {
                // Reconstruct from the numeric code, excluding caller-supplied context.
                let _ = write!(
                    description,
                    " os_error={code} ({})",
                    io::Error::from_raw_os_error(code)
                );
            }
            current = error
                .get_ref()
                .map(|error| error as &dyn Error)
                .or_else(|| error.source());
        } else {
            if let Some(error) = error.downcast_ref::<hyper::Error>() {
                let _ = write!(
                    description,
                    " http_error=true parse={} incomplete={} closed={} timeout={} user={}",
                    error.is_parse(),
                    error.is_incomplete_message(),
                    error.is_closed(),
                    error.is_timeout(),
                    error.is_user()
                );
            } else if let Some(error) = error.downcast_ref::<tokio_rustls::rustls::Error>() {
                let category = match error {
                    tokio_rustls::rustls::Error::InvalidCertificate(_) => "invalid certificate",
                    tokio_rustls::rustls::Error::NoCertificatesPresented => "missing certificate",
                    tokio_rustls::rustls::Error::AlertReceived(_) => "peer alert",
                    _ => "TLS protocol or configuration error",
                };
                let _ = write!(description, " tls_error={category}");
            } else if let Some(error) = error.downcast_ref::<tokio::task::JoinError>() {
                let _ = write!(
                    description,
                    " task_error=true panic={} cancelled={}",
                    error.is_panic(),
                    error.is_cancelled()
                );
            }
            current = error.source();
        }
    }
    if description.is_empty() {
        description.push_str(" non-OS error; unstructured details withheld");
    }
    description
}

/// Log a failed operation with safe diagnostics. Operation names must be static.
pub fn error(operation: &'static str, error: &(dyn Error + 'static)) {
    log::error!("{operation} failed:{}", summary(error));
}

#[cfg(test)]
mod tests {
    use super::summary;
    use std::io;

    #[test]
    fn wrapped_os_errors_remain_visible_without_exposing_custom_messages() {
        let error = io::Error::other(io::Error::from_raw_os_error(24));
        assert!(summary(&error).contains("os_error=24"));
        let error = io::Error::other("secret token and destination must not be logged");
        let message = summary(&error);
        assert!(message.contains("kind=Other"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("destination"));
    }
}
