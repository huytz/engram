use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::SdkError;
use aws_smithy_http_client::tls;
use base64::Engine;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use std::path::PathBuf;

const ROOTS_TTL: std::time::Duration = std::time::Duration::from_secs(86400);

/// Native roots cached as PEM for a day: reading the macOS keychain costs ~70ms per run
fn trust_store() -> tls::TrustStore {
    let p = crate::config::home().join("roots.pem");
    let fresh = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or_default() < ROOTS_TTL)
        .unwrap_or(false);
    if fresh
        && let Ok(pem) = std::fs::read(&p)
        && !pem.is_empty()
        && CertificateDer::pem_slice_iter(&pem).all(|c| c.is_ok())
    {
        return tls::TrustStore::empty()
            .with_native_roots(false)
            .with_pem_certificate(pem);
    }
    // Keep only certs rustls accepts: the SDK panics on any cert it cannot add
    let mut check = rustls::RootCertStore::empty();
    let mut pem = String::new();
    for c in rustls_native_certs::load_native_certs().certs {
        if check.add(c.clone()).is_ok() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(c.as_ref());
            pem.push_str("-----BEGIN CERTIFICATE-----\n");
            for line in b64.as_bytes().chunks(64) {
                pem.push_str(std::str::from_utf8(line).unwrap_or_default());
                pem.push('\n');
            }
            pem.push_str("-----END CERTIFICATE-----\n");
        }
    }
    if pem.is_empty() {
        return tls::TrustStore::default();
    }
    let _ = crate::config::write_private(&p, pem.as_bytes());
    tls::TrustStore::empty()
        .with_native_roots(false)
        .with_pem_certificate(pem)
}

/// Largest object engram will buffer; bigger ones are refused
const MAX_OBJECT: u64 = 256 * 1024 * 1024;

pub enum Fetched {
    NotModified,
    Missing,
    Found { data: Vec<u8>, etag: String },
}

pub enum Backend {
    File { root: PathBuf },
    S3 { client: Client, bucket: String },
}

impl Backend {
    pub async fn open(cfg: &crate::config::Config) -> Result<Backend> {
        if let Some(dir) = cfg.remote.strip_prefix("file://") {
            let root = PathBuf::from(dir);
            std::fs::create_dir_all(&root)?;
            return Ok(Backend::File { root });
        }
        let bucket = cfg
            .remote
            .strip_prefix("s3://")
            .ok_or_else(|| anyhow!("remote must be s3://bucket or file:///dir"))?
            .trim_end_matches('/')
            .to_string();
        let region = Region::new(cfg.region.clone().unwrap_or_else(|| "auto".into()));
        // R2 rejects the SDK's default streaming checksums
        let builder = match (&cfg.access_key_id, &cfg.secret_access_key, &cfg.endpoint) {
            // Static keys + endpoint: skip the default provider chain (env, profile files, IMDS probes)
            (Some(ak), Some(sk), Some(ep)) => aws_sdk_s3::config::Builder::new()
                .behavior_version(aws_config::BehaviorVersion::latest())
                .region(region)
                .credentials_provider(Credentials::new(ak, sk, None, None, "engram"))
                .endpoint_url(ep),
            _ => {
                let mut loader =
                    aws_config::defaults(aws_config::BehaviorVersion::latest()).region(region);
                if let (Some(ak), Some(sk)) = (&cfg.access_key_id, &cfg.secret_access_key) {
                    loader =
                        loader.credentials_provider(Credentials::new(ak, sk, None, None, "engram"));
                }
                if let Some(ep) = &cfg.endpoint {
                    loader = loader.endpoint_url(ep);
                }
                aws_sdk_s3::config::Builder::from(&loader.load().await)
            }
        };
        let tls = tls::TlsContext::builder()
            .with_trust_store(trust_store())
            .build()
            .context("tls context")?;
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(tls::Provider::Rustls(
                tls::rustls_provider::CryptoMode::AwsLc,
            ))
            .tls_context(tls)
            .build_https();
        let s3cfg = builder
            .http_client(http)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        Ok(Backend::S3 {
            client: Client::from_conf(s3cfg),
            bucket,
        })
    }

    pub async fn get(&self, key: &str, if_none_match: Option<&str>) -> Result<Fetched> {
        match self {
            Backend::File { root } => {
                let p = root.join(key);
                if std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX_OBJECT) {
                    bail!("{key} larger than {MAX_OBJECT} bytes");
                }
                let data = match std::fs::read(&p) {
                    Ok(d) => d,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Fetched::Missing);
                    }
                    Err(e) => return Err(e.into()),
                };
                let etag = crate::crypto::sha256_hex(&data);
                if if_none_match == Some(etag.as_str()) {
                    return Ok(Fetched::NotModified);
                }
                Ok(Fetched::Found { data, etag })
            }
            Backend::S3 { client, bucket } => {
                let mut req = client.get_object().bucket(bucket).key(key);
                if let Some(t) = if_none_match {
                    req = req.if_none_match(t);
                }
                match req.send().await {
                    Ok(out) => {
                        if out.content_length().is_some_and(|n| n as u64 > MAX_OBJECT) {
                            bail!("{key} larger than {MAX_OBJECT} bytes");
                        }
                        let etag = out.e_tag().unwrap_or_default().to_string();
                        let data = out
                            .body
                            .collect()
                            .await
                            .context("read body")?
                            .into_bytes()
                            .to_vec();
                        Ok(Fetched::Found { data, etag })
                    }
                    Err(SdkError::ServiceError(se)) => {
                        if se.raw().status().as_u16() == 304 {
                            return Ok(Fetched::NotModified);
                        }
                        if se.err().is_no_such_key() || se.raw().status().as_u16() == 404 {
                            return Ok(Fetched::Missing);
                        }
                        bail!("s3 get {key}: {}", se.err())
                    }
                    Err(e) => bail!("s3 get {key}: {e}"),
                }
            }
        }
    }

    pub async fn put(&self, key: &str, data: Vec<u8>) -> Result<()> {
        match self {
            Backend::File { root } => {
                let p = root.join(key);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = p.with_extension("tmp");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(tmp, p)?;
                Ok(())
            }
            Backend::S3 { client, bucket } => {
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(data.into())
                    .send()
                    .await
                    .map_err(|e| anyhow!("s3 put {key}: {}", e.into_service_error()))?;
                Ok(())
            }
        }
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        match self {
            Backend::File { root } => {
                let p = root.join(key);
                match std::fs::remove_file(&p) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                }
                // S3 has no directories: prune empty parents so LIST matches
                let mut d = p.parent();
                while let Some(dir) = d.filter(|d| d.starts_with(root) && *d != root.as_path()) {
                    if std::fs::remove_dir(dir).is_err() {
                        break;
                    }
                    d = dir.parent();
                }
                Ok(())
            }
            Backend::S3 { client, bucket } => {
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| anyhow!("s3 delete {key}: {}", e.into_service_error()))?;
                Ok(())
            }
        }
    }

    /// Immediate child names under `prefix` (S3 delimiter listing, no recursion into blobs)
    pub async fn list_children(&self, prefix: &str) -> Result<Vec<String>> {
        match self {
            Backend::File { root } => {
                let base = root.join(prefix);
                let Ok(rd) = std::fs::read_dir(&base) else {
                    return Ok(vec![]);
                };
                Ok(rd
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect())
            }
            Backend::S3 { client, bucket } => {
                let mut out = vec![];
                let mut token = None;
                loop {
                    let mut req = client
                        .list_objects_v2()
                        .bucket(bucket)
                        .prefix(prefix)
                        .delimiter("/");
                    if let Some(t) = &token {
                        req = req.continuation_token(t);
                    }
                    let resp = req
                        .send()
                        .await
                        .map_err(|e| anyhow!("s3 list: {}", e.into_service_error()))?;
                    for cp in resp.common_prefixes() {
                        if let Some(name) = cp
                            .prefix()
                            .and_then(|k| k.strip_prefix(prefix))
                            .map(|k| k.trim_end_matches('/'))
                            .filter(|k| !k.is_empty())
                        {
                            out.push(name.to_string());
                        }
                    }
                    match resp.next_continuation_token() {
                        Some(t) => token = Some(t.to_string()),
                        None => break,
                    }
                }
                Ok(out)
            }
        }
    }

    /// All keys under `prefix`
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        match self {
            Backend::File { root } => {
                let base = root.join(prefix);
                let mut out = vec![];
                if !base.exists() {
                    return Ok(out);
                }
                for e in walkdir::WalkDir::new(&base)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if e.file_type().is_file() {
                        let rel = e
                            .path()
                            .strip_prefix(root)?
                            .to_string_lossy()
                            .replace('\\', "/");
                        out.push(rel);
                    }
                }
                Ok(out)
            }
            Backend::S3 { client, bucket } => {
                let mut out = vec![];
                let mut token = None;
                loop {
                    let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
                    if let Some(t) = &token {
                        req = req.continuation_token(t);
                    }
                    let resp = req
                        .send()
                        .await
                        .map_err(|e| anyhow!("s3 list: {}", e.into_service_error()))?;
                    for o in resp.contents() {
                        if let Some(k) = o.key() {
                            out.push(k.to_string());
                        }
                    }
                    match resp.next_continuation_token() {
                        Some(t) => token = Some(t.to_string()),
                        None => break,
                    }
                }
                Ok(out)
            }
        }
    }
}
