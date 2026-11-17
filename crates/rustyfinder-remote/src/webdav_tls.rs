//! HTTPS that accepts one certificate the user chose to trust, checked on
//! the very connection that carries the requests.
//!
//! macOS refuses many self-signed certificates even as a trusted root (for
//! example any valid for more than 825 days), so "always trust" cannot be
//! built from root certificates. Instead the system's verdict is set aside
//! and the certificate the server presents must be exactly the trusted one,
//! by its SHA-256 fingerprint.

use std::fmt;
use std::io::{Read, Write};

use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, Transport, TransportAdapter,
};
use ureq::Error;

use crate::trust;

/// Wraps the TCP connection in TLS and checks the certificate's fingerprint.
pub struct PinnedTls {
    pub fingerprint: String,
}

impl fmt::Debug for PinnedTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedTls").finish()
    }
}

impl<In: Transport> Connector<In> for PinnedTls {
    type Out = Either<In, PinnedTransport>;

    fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, Error> {
        let Some(transport) = chained else { return Ok(None) };
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()?;
        let host = details.uri.host().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string();
        let mut adapter = TransportAdapter::new(transport.boxed(), details.current_time.clone());
        adapter.set_timeout(details.timeout);
        let stream = connector.connect(&host, adapter).map_err(|e| match e {
            native_tls::HandshakeError::Failure(e) => Error::NativeTls(e),
            native_tls::HandshakeError::WouldBlock(_) => Error::Tls("The TLS handshake did not finish."),
        })?;
        let presented = stream.peer_certificate().ok().flatten().and_then(|c| c.to_der().ok());
        if presented.map(|der| trust::fingerprint(&der)).as_deref() != Some(self.fingerprint.as_str()) {
            return Err(Error::Tls("The certificate is not the one you chose to trust."));
        }
        let buffers = LazyBuffers::new(details.config.input_buffer_size(), details.config.output_buffer_size());
        Ok(Some(Either::B(PinnedTransport { buffers, stream })))
    }
}

pub struct PinnedTransport {
    buffers: LazyBuffers,
    stream: native_tls::TlsStream<TransportAdapter>,
}

impl fmt::Debug for PinnedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedTransport").finish()
    }
}

impl Transport for PinnedTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        self.stream.write_all(output)?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        self.stream.get_mut().set_timeout(timeout);
        let input = self.buffers.input_append_buf();
        let amount = self.stream.read(input)?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}
