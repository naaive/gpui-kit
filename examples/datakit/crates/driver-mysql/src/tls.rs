use datakit_driver::SslMode;
use mysql_async::SslOpts;

/// The TLS options for `ssl_mode`; `None` for a connection without TLS.
///
/// `Prefer` and `Require` encrypt without verifying who is on the other end,
/// as libpq's modes of the same name do; `Prefer` additionally falls back to
/// a plain connection when the server has no TLS, which the caller does by
/// connecting again. `VerifyFull` checks the certificate against the Mozilla
/// roots `webpki-roots` carries and the host name against the certificate.
pub(crate) fn ssl_opts(ssl_mode: SslMode) -> Option<SslOpts> {
    if ssl_mode == SslMode::Disable {
        return None;
    }
    install_crypto_provider();
    let opts = SslOpts::default();
    Some(match ssl_mode {
        SslMode::VerifyFull => opts,
        _ => opts
            .with_danger_accept_invalid_certs(true)
            .with_danger_skip_domain_validation(true),
    })
}

/// Make `ring` the process's TLS provider unless one is installed.
///
/// `mysql_async` builds its TLS configuration from the process default, and
/// rustls cannot pick one by itself when an application links more than one
/// provider, as DataKit does through other crates.
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Losing a race to another installer is fine: one is installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
