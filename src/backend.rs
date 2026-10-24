use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::SdkError;
use std::path::PathBuf;

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
        let region = cfg.region.clone().unwrap_or_else(|| "auto".into());
        let mut loader =
            aws_config::defaults(aws_config::BehaviorVersion::latest()).region(Region::new(region));
        if let (Some(ak), Some(sk)) = (&cfg.access_key_id, &cfg.secret_access_key) {
            loader = loader.credentials_provider(Credentials::new(ak, sk, None, None, "engram"));
        }
        if let Some(ep) = &cfg.endpoint {
            loader = loader.endpoint_url(ep);
        }
        let sdk = loader.load().await;
        // R2 rejects the SDK's default streaming checksums
        let s3cfg = aws_sdk_s3::config::Builder::from(&sdk)
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

    pub async fn exists(&self, key: &str) -> Result<bool> {
        match self {
            Backend::File { root } => Ok(root.join(key).exists()),
            Backend::S3 { client, bucket } => {
                match client.head_object().bucket(bucket).key(key).send().await {
                    Ok(_) => Ok(true),
                    Err(SdkError::ServiceError(se)) if se.err().is_not_found() => Ok(false),
                    Err(e) => bail!("s3 head {key}: {e}"),
                }
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
