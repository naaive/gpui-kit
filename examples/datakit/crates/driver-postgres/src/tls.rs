use std::sync::Arc;

use anyhow::{Context as _, Result};
use datakit_driver::SslMode;
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio_postgres::{CancelToken, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

/// The TLS a connection was made with, kept so a cancel request — which is a
/// new connection to the server — uses the same.
#[derive(Clone)]
pub(crate) enum Tls {
    None,
    Rustls(MakeRustlsConnect),
}

impl Tls {
    pub(crate) fn new(ssl_mode: SslMode) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .context("TLS is not available")?;
        let config = match ssl_mode {
            SslMode::Disable => return Ok(Tls::None),
            // libpq's `prefer` and `require` encrypt without verifying who is
            // on the other end; `verify-full` is the mode that does.
            SslMode::Prefer | SslMode::Require => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Unverified(provider)))
                .with_no_client_auth(),
            SslMode::VerifyFull => {
                let mut roots = RootCertStore::empty();
                let native = rustls_native_certs::load_native_certs();
                for error in &native.errors {
                    tracing::warn!("skipping a system certificate: {error}");
                }
                roots.add_parsable_certificates(native.certs);
                builder.with_root_certificates(roots).with_no_client_auth()
            }
        };
        Ok(Tls::Rustls(MakeRustlsConnect::new(config)))
    }

    pub(crate) async fn cancel(&self, token: &CancelToken) -> Result<()> {
        match self {
            Tls::None => token.cancel_query(NoTls).await?,
            Tls::Rustls(tls) => token.cancel_query(tls.clone()).await?,
        }
        Ok(())
    }
}

/// Accepts any certificate, and still checks that the handshake was signed by
/// it, which is what `sslmode=require` promises.
#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
