use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream},
    sync::{Arc, Mutex, Once},
    thread,
    time::Duration,
};

use ordo_protocol::{
    Envelope, ExecutionTarget, NatKind, OrdoMessage, PeerPresence, RouteDirective, TrafficClass,
    TransportKind,
};
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig, Endpoint, ServerConfig};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const DEFAULT_INSECURE_QUIC_SERVER_NAME: &str = "localhost";
static RUSTLS_PROVIDER: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeProfile {
    InProcess,
    HybridPqNoise,
    NoiseFallback,
    RelayHybridPqNoise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    DirectStream,
    PullManifest,
    ChunkRepair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshRoutePlan {
    pub target: ExecutionTarget,
    pub transport: TransportKind,
    pub handshake: HandshakeProfile,
    pub relay_required: bool,
    pub delivery_mode: DeliveryMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportReceipt {
    pub transport: TransportKind,
    pub delivered_to: Vec<String>,
    pub loopback: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuicEndpointDescriptor {
    address: SocketAddr,
    server_name: String,
    fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuicServerInfo {
    pub endpoint: Endpoint,
    pub fingerprint: String,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("no delivery target is available for the selected route")]
    NoDeliveryTarget,
    #[error("remote peer does not advertise a tcp endpoint")]
    MissingTcpEndpoint,
    #[error("invalid tcp endpoint '{0}'")]
    InvalidEndpoint(String),
    #[error("unsupported transport for this adapter: {0:?}")]
    UnsupportedTransport(TransportKind),
    #[error("transport I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport serialization failed: {0}")]
    Serialization(String),
    #[error("QUIC transport failed: {0}")]
    Quic(String),
    #[error("QUIC worker thread panicked")]
    QuicThreadPanic,
}

pub trait TransportAdapter: std::fmt::Debug + Send {
    fn send(
        &mut self,
        plan: &MeshRoutePlan,
        local: &PeerPresence,
        remote: Option<&PeerPresence>,
        envelope: &Envelope<OrdoMessage>,
    ) -> Result<TransportReceipt, TransportError>;
}

#[derive(Debug, Default)]
pub struct SimulatedTransportAdapter;

pub struct DefaultTransportAdapter {
    connect_timeout: Duration,
    fallback: SimulatedTransportAdapter,
    quic_connections: Mutex<HashMap<SocketAddr, quinn::Connection>>,
}

impl std::fmt::Debug for DefaultTransportAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultTransportAdapter")
            .field("connect_timeout", &self.connect_timeout)
            .finish()
    }
}

impl Default for DefaultTransportAdapter {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(3),
            fallback: SimulatedTransportAdapter,
            quic_connections: Mutex::new(HashMap::new()),
        }
    }
}

impl DefaultTransportAdapter {
    pub fn with_connect_timeout(connect_timeout: Duration) -> Self {
        Self {
            connect_timeout,
            ..Self::default()
        }
    }
}

impl TransportAdapter for SimulatedTransportAdapter {
    fn send(
        &mut self,
        plan: &MeshRoutePlan,
        local: &PeerPresence,
        remote: Option<&PeerPresence>,
        _envelope: &Envelope<OrdoMessage>,
    ) -> Result<TransportReceipt, TransportError> {
        match plan.target {
            ExecutionTarget::LocalOnly => Ok(TransportReceipt {
                transport: plan.transport.clone(),
                delivered_to: vec![local.label.clone()],
                loopback: true,
                description: "loopback delivery via local transport adapter".to_string(),
            }),
            ExecutionTarget::BestPeer | ExecutionTarget::SpecificPeer(_) => {
                let peer = remote.ok_or(TransportError::NoDeliveryTarget)?;
                Ok(TransportReceipt {
                    transport: plan.transport.clone(),
                    delivered_to: vec![peer.label.clone()],
                    loopback: false,
                    description: format!("simulated peer delivery to {}", peer.label),
                })
            }
            ExecutionTarget::Broadcast => Ok(TransportReceipt {
                transport: plan.transport.clone(),
                delivered_to: vec!["broadcast".to_string()],
                loopback: false,
                description: "simulated broadcast delivery".to_string(),
            }),
        }
    }
}

impl TransportAdapter for DefaultTransportAdapter {
    fn send(
        &mut self,
        plan: &MeshRoutePlan,
        local: &PeerPresence,
        remote: Option<&PeerPresence>,
        envelope: &Envelope<OrdoMessage>,
    ) -> Result<TransportReceipt, TransportError> {
        match plan.transport {
            TransportKind::TcpNoise => self.send_tcp(plan, local, remote, envelope),
            TransportKind::Quic => {
                if let Some(receipt) = self.try_send_quic(plan, local, remote, envelope)? {
                    Ok(receipt)
                } else {
                    self.fallback.send(plan, local, remote, envelope)
                }
            }
            _ => self.fallback.send(plan, local, remote, envelope),
        }
    }
}

impl DefaultTransportAdapter {
    fn send_tcp(
        &mut self,
        plan: &MeshRoutePlan,
        local: &PeerPresence,
        remote: Option<&PeerPresence>,
        envelope: &Envelope<OrdoMessage>,
    ) -> Result<TransportReceipt, TransportError> {
        if matches!(plan.target, ExecutionTarget::LocalOnly) {
            return self.fallback.send(plan, local, remote, envelope);
        }

        let peer = remote.ok_or(TransportError::NoDeliveryTarget)?;
        let endpoint = parse_tcp_endpoint(peer)?;
        let mut stream = TcpStream::connect_timeout(&endpoint, self.connect_timeout)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(self.connect_timeout))?;
        let body_len = write_framed_envelope(&mut stream, envelope)?;
        let _ = stream.shutdown(Shutdown::Write);

        Ok(TransportReceipt {
            transport: plan.transport.clone(),
            delivered_to: vec![peer.label.clone()],
            loopback: false,
            description: format!(
                "tcp framed delivery to {} via {} ({} bytes)",
                peer.label, endpoint, body_len
            ),
        })
    }

    fn try_send_quic(
        &mut self,
        plan: &MeshRoutePlan,
        local: &PeerPresence,
        remote: Option<&PeerPresence>,
        envelope: &Envelope<OrdoMessage>,
    ) -> Result<Option<TransportReceipt>, TransportError> {
        if matches!(plan.target, ExecutionTarget::LocalOnly) {
            return Ok(Some(self.fallback.send(plan, local, remote, envelope)?));
        }

        let peer = remote.ok_or(TransportError::NoDeliveryTarget)?;
        let Some(endpoint) = parse_quic_endpoint(peer)? else {
            return Ok(None);
        };

        let peer_label = peer.label.clone();
        let delivery_addr = endpoint.address;
        let is_pinned = endpoint.fingerprint.is_some();

        // Try reusing a cached connection first.
        if let Some(body_len) = self.try_cached_quic_send(&endpoint, envelope)? {
            let verification_mode = if is_pinned {
                "fingerprint-pinned"
            } else {
                "insecure local verifier"
            };
            return Ok(Some(TransportReceipt {
                transport: plan.transport.clone(),
                delivered_to: vec![peer_label.clone()],
                loopback: false,
                description: format!(
                    "direct quic delivery to {} via {} ({} bytes, {}, cached)",
                    peer_label, delivery_addr, body_len, verification_mode
                ),
            }));
        }

        let envelope = envelope.clone();
        let timeout = self.connect_timeout;
        let connections = self.quic_connections.lock().unwrap().clone();
        let result =
            thread::spawn(move || run_quic_client(&endpoint, &envelope, timeout, &connections))
                .join()
                .map_err(|_| TransportError::QuicThreadPanic)??;

        // Cache the new connection if one was returned.
        if let Some(connection) = result.connection {
            self.quic_connections
                .lock()
                .unwrap()
                .insert(delivery_addr, connection);
        }

        let verification_mode = if is_pinned {
            "fingerprint-pinned"
        } else {
            "insecure local verifier"
        };
        Ok(Some(TransportReceipt {
            transport: plan.transport.clone(),
            delivered_to: vec![peer_label.clone()],
            loopback: false,
            description: format!(
                "direct quic delivery to {} via {} ({} bytes, {})",
                peer_label, delivery_addr, result.body_len, verification_mode
            ),
        }))
    }

    fn try_cached_quic_send(
        &self,
        endpoint: &QuicEndpointDescriptor,
        envelope: &Envelope<OrdoMessage>,
    ) -> Result<Option<usize>, TransportError> {
        let connection = {
            let connections = self.quic_connections.lock().unwrap();
            connections.get(&endpoint.address).cloned()
        };

        let Some(connection) = connection else {
            return Ok(None);
        };

        // If the connection is closed, drop it from cache and return None.
        if connection.close_reason().is_some() {
            self.quic_connections
                .lock()
                .unwrap()
                .remove(&endpoint.address);
            return Ok(None);
        }

        let timeout = self.connect_timeout;
        let envelope = envelope.clone();
        match thread::spawn(move || send_on_quic_connection(&connection, &envelope, timeout))
            .join()
            .map_err(|_| TransportError::QuicThreadPanic)?
        {
            Ok(body_len) => Ok(Some(body_len)),
            Err(_) => {
                self.quic_connections
                    .lock()
                    .unwrap()
                    .remove(&endpoint.address);
                Ok(None)
            }
        }
    }
}

pub fn write_framed_envelope<W: Write>(
    writer: &mut W,
    envelope: &Envelope<OrdoMessage>,
) -> Result<usize, TransportError> {
    let body = serde_json::to_vec(envelope)
        .map_err(|err| TransportError::Serialization(err.to_string()))?;
    let length = u32::try_from(body.len()).map_err(|_| {
        TransportError::Serialization("envelope exceeds 4 GiB frame limit".to_string())
    })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(body.len())
}

pub fn read_framed_envelope<R: Read>(
    reader: &mut R,
) -> Result<Envelope<OrdoMessage>, TransportError> {
    let mut length_bytes = [0u8; 4];
    reader.read_exact(&mut length_bytes)?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|err| TransportError::Serialization(err.to_string()))
}

pub async fn write_async_framed_envelope<W>(
    writer: &mut W,
    envelope: &Envelope<OrdoMessage>,
) -> Result<usize, TransportError>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(envelope)
        .map_err(|err| TransportError::Serialization(err.to_string()))?;
    let length = u32::try_from(body.len()).map_err(|_| {
        TransportError::Serialization("envelope exceeds 4 GiB frame limit".to_string())
    })?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(body.len())
}

pub async fn read_async_framed_envelope<R>(
    reader: &mut R,
) -> Result<Envelope<OrdoMessage>, TransportError>
where
    R: AsyncRead + Unpin,
{
    let mut length_bytes = [0u8; 4];
    reader.read_exact(&mut length_bytes).await?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|err| TransportError::Serialization(err.to_string()))
}

/// Creates a QUIC server endpoint for insecure local/dev use. Does not return a
/// fingerprint because the client side skips verification anyway.
pub fn make_local_demo_quic_server_endpoint(
    bind_addr: SocketAddr,
) -> Result<Endpoint, TransportError> {
    let info = make_quic_server_endpoint(bind_addr)?;
    Ok(info.endpoint)
}

/// Creates a QUIC server endpoint and returns both the endpoint and the SHA-256
/// fingerprint of the generated self-signed certificate. Peers can pin the
/// fingerprint in their `quic://host:port#sha256:<hex>` endpoint to verify the
/// server identity without a CA.
pub fn make_quic_server_endpoint(bind_addr: SocketAddr) -> Result<QuicServerInfo, TransportError> {
    ensure_rustls_provider();
    let certified =
        rcgen::generate_simple_self_signed(vec![DEFAULT_INSECURE_QUIC_SERVER_NAME.to_string()])
            .map_err(|err| TransportError::Quic(err.to_string()))?;
    let cert_der = CertificateDer::from(certified.cert);
    let fingerprint = compute_cert_fingerprint(&cert_der);
    let private_key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let mut server_config = ServerConfig::with_single_cert(vec![cert_der], private_key.into())
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    if let Some(transport_config) = Arc::get_mut(&mut server_config.transport) {
        transport_config.max_concurrent_uni_streams(4_u8.into());
        transport_config.max_concurrent_bidi_streams(4_u8.into());
    }
    let endpoint = Endpoint::server(server_config, bind_addr)
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    Ok(QuicServerInfo {
        endpoint,
        fingerprint,
    })
}

fn compute_cert_fingerprint(cert_der: &CertificateDer<'_>) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert_der.as_ref());
    hex_encode(digest.as_ref())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect()
}

pub fn plan_mesh_route(directive: &RouteDirective, peers: &[PeerPresence]) -> MeshRoutePlan {
    let selected_peer = select_peer(directive, peers);
    let relay_required = selected_peer
        .map(|peer| matches!(peer.nat_kind, NatKind::Symmetric | NatKind::RelayOnly))
        .unwrap_or(false);

    let transport = match directive.execution_target {
        ExecutionTarget::LocalOnly => TransportKind::InProcess,
        ExecutionTarget::Broadcast if peers.is_empty() => TransportKind::InProcess,
        ExecutionTarget::Broadcast => TransportKind::Quic,
        _ if relay_required => TransportKind::RelayQuic,
        _ => selected_peer
            .and_then(|peer| {
                peer.transports
                    .iter()
                    .find(|transport| {
                        matches!(
                            transport,
                            TransportKind::Quic
                                | TransportKind::TcpNoise
                                | TransportKind::RelayQuic
                        )
                    })
                    .cloned()
            })
            .unwrap_or(TransportKind::RelayQuic),
    };

    let handshake = match transport {
        TransportKind::InProcess => HandshakeProfile::InProcess,
        TransportKind::Quic if directive.prefer_pq => HandshakeProfile::HybridPqNoise,
        TransportKind::RelayQuic if directive.prefer_pq => HandshakeProfile::RelayHybridPqNoise,
        _ => HandshakeProfile::NoiseFallback,
    };

    let delivery_mode = match directive.traffic_class {
        TrafficClass::Update => DeliveryMode::PullManifest,
        TrafficClass::Replication => DeliveryMode::ChunkRepair,
        _ => DeliveryMode::DirectStream,
    };

    MeshRoutePlan {
        target: directive.execution_target.clone(),
        transport,
        handshake,
        relay_required,
        delivery_mode,
    }
}

fn select_peer<'a>(
    directive: &RouteDirective,
    peers: &'a [PeerPresence],
) -> Option<&'a PeerPresence> {
    match &directive.execution_target {
        ExecutionTarget::SpecificPeer(id) => peers.iter().find(|peer| &peer.id == id),
        ExecutionTarget::BestPeer => peers.iter().find(|peer| {
            directive
                .required_capabilities
                .iter()
                .all(|required| peer.capabilities.iter().any(|cap| cap == required))
        }),
        ExecutionTarget::LocalOnly | ExecutionTarget::Broadcast => None,
    }
}

fn parse_tcp_endpoint(peer: &PeerPresence) -> Result<SocketAddr, TransportError> {
    let raw = peer
        .endpoints
        .iter()
        .find(|endpoint| endpoint.starts_with("tcp://") || endpoint.starts_with("tcp+noise://"))
        .ok_or(TransportError::MissingTcpEndpoint)?;

    let addr = raw
        .strip_prefix("tcp://")
        .or_else(|| raw.strip_prefix("tcp+noise://"))
        .ok_or_else(|| TransportError::InvalidEndpoint(raw.clone()))?;
    addr.parse::<SocketAddr>()
        .map_err(|_| TransportError::InvalidEndpoint(raw.clone()))
}

fn parse_quic_endpoint(
    peer: &PeerPresence,
) -> Result<Option<QuicEndpointDescriptor>, TransportError> {
    // Prefer quic+insecure:// for dev, then quic:// for secure.
    let raw = peer
        .endpoints
        .iter()
        .find(|endpoint| endpoint.starts_with("quic+insecure://"))
        .or_else(|| {
            peer.endpoints
                .iter()
                .find(|endpoint| endpoint.starts_with("quic://"))
        });

    let Some(raw) = raw else {
        return Ok(None);
    };

    if let Some(addr) = raw.strip_prefix("quic+insecure://") {
        let (addr, server_name) = if let Some((socket, server_name)) = addr.split_once('#') {
            (socket, server_name)
        } else {
            (addr, DEFAULT_INSECURE_QUIC_SERVER_NAME)
        };

        let address = addr
            .parse::<SocketAddr>()
            .map_err(|_| TransportError::InvalidEndpoint(raw.clone()))?;

        return Ok(Some(QuicEndpointDescriptor {
            address,
            server_name: server_name.to_string(),
            fingerprint: None,
        }));
    }

    if let Some(addr) = raw.strip_prefix("quic://") {
        let (addr, fragment) = if let Some((socket, fragment)) = addr.split_once('#') {
            (socket, Some(fragment))
        } else {
            (addr, None)
        };

        let fingerprint = fragment
            .and_then(|fragment| fragment.strip_prefix("sha256:"))
            .map(str::to_string);

        if fingerprint.is_none() {
            // quic:// without a pinned fingerprint is not deliverable yet.
            return Ok(None);
        }

        let address = addr
            .parse::<SocketAddr>()
            .map_err(|_| TransportError::InvalidEndpoint(raw.clone()))?;

        return Ok(Some(QuicEndpointDescriptor {
            address,
            server_name: DEFAULT_INSECURE_QUIC_SERVER_NAME.to_string(),
            fingerprint,
        }));
    }

    Ok(None)
}

struct QuicSendResult {
    body_len: usize,
    connection: Option<quinn::Connection>,
}

fn run_quic_client(
    endpoint: &QuicEndpointDescriptor,
    envelope: &Envelope<OrdoMessage>,
    connect_timeout: Duration,
    _cached: &HashMap<SocketAddr, quinn::Connection>,
) -> Result<QuicSendResult, TransportError> {
    ensure_rustls_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| TransportError::Quic(err.to_string()))?;

    let endpoint_clone = endpoint.clone();
    let envelope_clone = envelope.clone();
    runtime.block_on(async move {
        let bind_addr = wildcard_client_bind(endpoint_clone.address);
        let mut client =
            Endpoint::client(bind_addr).map_err(|err| TransportError::Quic(err.to_string()))?;

        let client_config = if let Some(fingerprint) = &endpoint_clone.fingerprint {
            pinned_quic_client_config(fingerprint)?
        } else {
            insecure_quic_client_config()?
        };
        client.set_default_client_config(client_config);

        let connecting = client
            .connect(endpoint_clone.address, endpoint_clone.server_name.as_str())
            .map_err(|err| TransportError::Quic(err.to_string()))?;
        let connection = tokio::time::timeout(connect_timeout, connecting)
            .await
            .map_err(|_| {
                TransportError::Quic(format!(
                    "timed out connecting to {}",
                    endpoint_clone.address
                ))
            })?
            .map_err(|err| TransportError::Quic(err.to_string()))?;

        let body_len =
            send_on_quic_connection_async(&connection, &envelope_clone, connect_timeout).await?;

        Ok(QuicSendResult {
            body_len,
            connection: Some(connection),
        })
    })
}

fn send_on_quic_connection(
    connection: &quinn::Connection,
    envelope: &Envelope<OrdoMessage>,
    timeout: Duration,
) -> Result<usize, TransportError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    let connection = connection.clone();
    let envelope = envelope.clone();
    runtime.block_on(
        async move { send_on_quic_connection_async(&connection, &envelope, timeout).await },
    )
}

async fn send_on_quic_connection_async(
    connection: &quinn::Connection,
    envelope: &Envelope<OrdoMessage>,
    timeout: Duration,
) -> Result<usize, TransportError> {
    let (mut send_stream, mut recv_stream) = tokio::time::timeout(timeout, connection.open_bi())
        .await
        .map_err(|_| TransportError::Quic("timed out opening QUIC stream".to_string()))?
        .map_err(|err| TransportError::Quic(err.to_string()))?;

    let body_len = tokio::time::timeout(
        timeout,
        write_async_framed_envelope(&mut send_stream, envelope),
    )
    .await
    .map_err(|_| TransportError::Quic("timed out writing QUIC frame".to_string()))??;

    send_stream
        .finish()
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    let mut ack = [0u8; 1];
    tokio::time::timeout(timeout, recv_stream.read_exact(&mut ack))
        .await
        .map_err(|_| TransportError::Quic("timed out waiting for QUIC ack".to_string()))?
        .map_err(|err| TransportError::Quic(err.to_string()))?;

    Ok(body_len)
}

fn wildcard_client_bind(remote: SocketAddr) -> SocketAddr {
    match remote.ip() {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

fn pinned_quic_client_config(fingerprint: &str) -> Result<ClientConfig, TransportError> {
    ensure_rustls_provider();
    let expected = hex_decode(fingerprint)
        .ok_or_else(|| TransportError::Quic(format!("invalid fingerprint hex: {fingerprint}")))?;
    let rustls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(FingerprintVerifier::new(expected))
        .with_no_client_auth();
    let quic_config = QuicClientConfig::try_from(rustls_config)
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    Ok(ClientConfig::new(Arc::new(quic_config)))
}

fn insecure_quic_client_config() -> Result<ClientConfig, TransportError> {
    ensure_rustls_provider();
    let rustls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    let quic_config = QuicClientConfig::try_from(rustls_config)
        .map_err(|err| TransportError::Quic(err.to_string()))?;
    Ok(ClientConfig::new(Arc::new(quic_config)))
}

fn ensure_rustls_provider() {
    RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
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
        rustls::crypto::verify_tls13_signature(
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

#[derive(Debug)]
struct FingerprintVerifier {
    expected_fingerprint: Vec<u8>,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl FingerprintVerifier {
    fn new(expected_fingerprint: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            expected_fingerprint,
            crypto_provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = ring::digest::digest(&ring::digest::SHA256, end_entity.as_ref());
        if actual.as_ref() == self.expected_fingerprint.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "certificate fingerprint mismatch: expected {}, got {}",
                hex_encode(&self.expected_fingerprint),
                hex_encode(actual.as_ref()),
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, sync::mpsc, thread, time::Duration};

    use super::{
        make_local_demo_quic_server_endpoint, make_quic_server_endpoint, plan_mesh_route,
        read_async_framed_envelope, read_framed_envelope, DefaultTransportAdapter, DeliveryMode,
        HandshakeProfile, SimulatedTransportAdapter, TransportAdapter,
    };
    use ordo_protocol::{
        CryptoSuite, Envelope, ExecutionTarget, NatKind, NodeId, OrdoMessage, PairingMode,
        PeerPresence, RouteDirective, TrafficClass, TransportKind, TrustTier,
    };

    fn symmetric_peer() -> PeerPresence {
        PeerPresence {
            id: NodeId::new(),
            label: "relay-me".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Symmetric,
            transports: vec![TransportKind::Quic, TransportKind::RelayQuic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec!["quic://relay-me".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        }
    }

    #[test]
    fn local_directive_stays_in_process() {
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Background,
                execution_target: ExecutionTarget::LocalOnly,
                required_capabilities: Vec::new(),
                prefer_pq: false,
                allow_relay_fallback: false,
            },
            &[],
        );

        assert_eq!(plan.transport, TransportKind::InProcess);
        assert_eq!(plan.handshake, HandshakeProfile::InProcess);
        assert_eq!(plan.delivery_mode, DeliveryMode::DirectStream);
    }

    #[test]
    fn symmetric_nat_uses_relay_hybrid_route() {
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Interactive,
                execution_target: ExecutionTarget::BestPeer,
                required_capabilities: vec!["filesystem.read_file".to_string()],
                prefer_pq: true,
                allow_relay_fallback: true,
            },
            &[symmetric_peer()],
        );

        assert_eq!(plan.transport, TransportKind::RelayQuic);
        assert_eq!(plan.handshake, HandshakeProfile::RelayHybridPqNoise);
        assert!(plan.relay_required);
    }

    #[test]
    fn simulated_adapter_loopbacks_local_delivery() {
        let mut adapter = SimulatedTransportAdapter;
        let local = PeerPresence {
            id: NodeId::new(),
            label: "local".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::LocalProcess,
            pairing_mode: PairingMode::LocalOnly,
            nat_kind: NatKind::OpenInternet,
            transports: vec![TransportKind::InProcess],
            crypto_suites: vec![CryptoSuite::InProcess],
            endpoints: vec!["inproc://local".to_string()],
            capabilities: vec![],
        };
        let receipt = adapter
            .send(
                &plan_mesh_route(
                    &RouteDirective {
                        traffic_class: TrafficClass::Background,
                        execution_target: ExecutionTarget::LocalOnly,
                        required_capabilities: Vec::new(),
                        prefer_pq: false,
                        allow_relay_fallback: false,
                    },
                    &[],
                ),
                &local,
                None,
                &Envelope::new(
                    local.id.clone(),
                    OrdoMessage::MemoryQuery {
                        query: "config".to_string(),
                    },
                ),
            )
            .expect("loopback receipt");
        assert!(receipt.loopback);
        assert_eq!(receipt.delivered_to, vec!["local".to_string()]);
    }

    #[test]
    fn tcp_noise_route_is_selected_when_peer_only_offers_tcp_noise() {
        let peer = PeerPresence {
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::TcpNoise],
            endpoints: vec!["tcp://127.0.0.1:40000".to_string()],
            ..symmetric_peer()
        };
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Interactive,
                execution_target: ExecutionTarget::BestPeer,
                required_capabilities: vec!["filesystem.read_file".to_string()],
                prefer_pq: false,
                allow_relay_fallback: false,
            },
            &[peer],
        );

        assert_eq!(plan.transport, TransportKind::TcpNoise);
        assert_eq!(plan.handshake, HandshakeProfile::NoiseFallback);
    }

    #[test]
    fn default_adapter_sends_tcp_framed_envelopes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind tcp listener");
        let address = listener.local_addr().expect("listener address");
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let envelope = read_framed_envelope(&mut stream).expect("read framed envelope");
            tx.send(envelope).expect("send envelope to test");
        });

        let local = PeerPresence {
            id: NodeId::new(),
            label: "local".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::LocalProcess,
            pairing_mode: PairingMode::LocalOnly,
            nat_kind: NatKind::OpenInternet,
            transports: vec![TransportKind::InProcess, TransportKind::TcpNoise],
            crypto_suites: vec![CryptoSuite::InProcess, CryptoSuite::NoiseX25519],
            endpoints: vec!["inproc://local".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let remote = PeerPresence {
            id: NodeId::new(),
            label: "tcp-peer".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::TcpNoise],
            crypto_suites: vec![CryptoSuite::NoiseX25519],
            endpoints: vec![format!("tcp://{address}")],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Interactive,
                execution_target: ExecutionTarget::BestPeer,
                required_capabilities: vec!["filesystem.read_file".to_string()],
                prefer_pq: false,
                allow_relay_fallback: false,
            },
            std::slice::from_ref(&remote),
        );
        let mut adapter = DefaultTransportAdapter::with_connect_timeout(Duration::from_secs(2));
        let envelope = Envelope::new(
            local.id.clone(),
            OrdoMessage::RequirementMessage {
                requirement: "read file config.json".to_string(),
            },
        );

        let receipt = adapter
            .send(&plan, &local, Some(&remote), &envelope)
            .expect("tcp delivery receipt");
        assert_eq!(receipt.transport, TransportKind::TcpNoise);
        assert_eq!(receipt.delivered_to, vec!["tcp-peer".to_string()]);
        assert!(receipt.description.contains("tcp framed delivery"));

        let received = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive framed envelope");
        match received.payload {
            OrdoMessage::RequirementMessage { requirement } => {
                assert_eq!(requirement, "read file config.json");
            }
            other => panic!("unexpected payload: {other:?}"),
        }

        server.join().expect("tcp server thread");
    }

    #[test]
    fn default_adapter_sends_quic_framed_envelopes_for_explicit_insecure_endpoints() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build quic runtime");
            runtime.block_on(async move {
                let endpoint = make_local_demo_quic_server_endpoint(
                    "127.0.0.1:0".parse().expect("quic bind address"),
                )
                .expect("create quic demo endpoint");
                ready_tx
                    .send(endpoint.local_addr().expect("quic local address"))
                    .expect("send quic listen address");

                let incoming = endpoint.accept().await.expect("accept quic connection");
                let connection = incoming.await.expect("finish quic connection");
                let (mut send_stream, mut recv_stream) =
                    connection.accept_bi().await.expect("accept bi stream");
                let envelope = read_async_framed_envelope(&mut recv_stream)
                    .await
                    .expect("read quic envelope");
                send_stream.write_all(&[1]).await.expect("send quic ack");
                send_stream.finish().expect("finish quic ack stream");
                msg_tx.send(envelope).expect("send envelope to test");
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
        });

        let address = ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive quic address");

        let local = PeerPresence {
            id: NodeId::new(),
            label: "local".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::LocalProcess,
            pairing_mode: PairingMode::LocalOnly,
            nat_kind: NatKind::OpenInternet,
            transports: vec![TransportKind::InProcess, TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::InProcess, CryptoSuite::HybridPqNoiseX25519],
            endpoints: vec!["inproc://local".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let remote = PeerPresence {
            id: NodeId::new(),
            label: "quic-peer".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec![format!("quic+insecure://{address}")],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Interactive,
                execution_target: ExecutionTarget::BestPeer,
                required_capabilities: vec!["filesystem.read_file".to_string()],
                prefer_pq: true,
                allow_relay_fallback: false,
            },
            std::slice::from_ref(&remote),
        );
        let mut adapter = DefaultTransportAdapter::with_connect_timeout(Duration::from_secs(3));
        let envelope = Envelope::new(
            local.id.clone(),
            OrdoMessage::RequirementMessage {
                requirement: "read file config.json over quic".to_string(),
            },
        );

        let receipt = adapter
            .send(&plan, &local, Some(&remote), &envelope)
            .expect("quic delivery receipt");
        assert_eq!(receipt.transport, TransportKind::Quic);
        assert_eq!(receipt.delivered_to, vec!["quic-peer".to_string()]);
        assert!(receipt.description.contains("direct quic delivery"));

        let received = msg_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive quic envelope");
        match received.payload {
            OrdoMessage::RequirementMessage { requirement } => {
                assert_eq!(requirement, "read file config.json over quic");
            }
            other => panic!("unexpected payload: {other:?}"),
        }

        server.join().expect("quic server thread");
    }

    #[test]
    fn default_adapter_sends_fingerprint_pinned_quic_envelopes() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build quic runtime");
            runtime.block_on(async move {
                let server_info =
                    make_quic_server_endpoint("127.0.0.1:0".parse().expect("quic bind address"))
                        .expect("create quic server endpoint");
                let addr = server_info
                    .endpoint
                    .local_addr()
                    .expect("quic local address");
                ready_tx
                    .send((addr, server_info.fingerprint))
                    .expect("send quic listen address and fingerprint");

                let incoming = server_info
                    .endpoint
                    .accept()
                    .await
                    .expect("accept quic connection");
                let connection = incoming.await.expect("finish quic connection");
                let (mut send_stream, mut recv_stream) =
                    connection.accept_bi().await.expect("accept bi stream");
                let envelope = read_async_framed_envelope(&mut recv_stream)
                    .await
                    .expect("read quic envelope");
                send_stream.write_all(&[1]).await.expect("send quic ack");
                send_stream.finish().expect("finish quic ack stream");
                msg_tx.send(envelope).expect("send envelope to test");
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
        });

        let (address, fingerprint) = ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive quic address and fingerprint");

        let local = PeerPresence {
            id: NodeId::new(),
            label: "local".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::LocalProcess,
            pairing_mode: PairingMode::LocalOnly,
            nat_kind: NatKind::OpenInternet,
            transports: vec![TransportKind::InProcess, TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::InProcess, CryptoSuite::HybridPqNoiseX25519],
            endpoints: vec!["inproc://local".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let remote = PeerPresence {
            id: NodeId::new(),
            label: "pinned-peer".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec![format!("quic://{address}#sha256:{fingerprint}")],
            capabilities: vec!["filesystem.read_file".to_string()],
        };
        let plan = plan_mesh_route(
            &RouteDirective {
                traffic_class: TrafficClass::Interactive,
                execution_target: ExecutionTarget::BestPeer,
                required_capabilities: vec!["filesystem.read_file".to_string()],
                prefer_pq: true,
                allow_relay_fallback: false,
            },
            std::slice::from_ref(&remote),
        );
        let mut adapter = DefaultTransportAdapter::with_connect_timeout(Duration::from_secs(3));
        let envelope = Envelope::new(
            local.id.clone(),
            OrdoMessage::RequirementMessage {
                requirement: "read file over fingerprint-pinned quic".to_string(),
            },
        );

        let receipt = adapter
            .send(&plan, &local, Some(&remote), &envelope)
            .expect("pinned quic delivery receipt");
        assert_eq!(receipt.transport, TransportKind::Quic);
        assert_eq!(receipt.delivered_to, vec!["pinned-peer".to_string()]);
        assert!(
            receipt.description.contains("fingerprint-pinned"),
            "receipt should mention fingerprint-pinned: {}",
            receipt.description
        );
        assert!(receipt.description.contains("direct quic delivery"));

        let received = msg_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive quic envelope");
        match received.payload {
            OrdoMessage::RequirementMessage { requirement } => {
                assert_eq!(requirement, "read file over fingerprint-pinned quic");
            }
            other => panic!("unexpected payload: {other:?}"),
        }

        server.join().expect("quic server thread");
    }
}
