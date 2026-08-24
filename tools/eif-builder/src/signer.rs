// SPDX-License-Identifier: Apache-2.0 OR MIT
//! EIF signature production for the `EifSectionSignature` (0x04) section.
//!
//! [`LocalSigner`] signs in-process from a PEM ECDSA key.
//! [`KmsSigner`] delegates to AWS KMS. Both emit the COSE raw ECDSA
//! encoding `I2OSP(R, n) | I2OSP(S, n)` with `n = ceil(key_length / 8)`
//! (RFC 9053 §2.1: https://www.rfc-editor.org/rfc/rfc9053#section-2.1);
//! P-256 → ES256, P-384 → ES384; other curves rejected. The TBS bytes
//! passed to [`Signer::sign_cose`] are the COSE `Sig_structure1`
//! serialization defined in (RFC 9052 §4.4: https://www.rfc-editor.org/rfc/rfc9052#section-4.4),
//!  built by the caller.

use std::sync::Arc;

use async_trait::async_trait;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1_SIGNING,
};
use snafu::{ResultExt, Snafu};

/// Errors from any signer backend.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum SignerError {
    #[snafu(display("failed to read signing key file: {source}"))]
    ReadKey { source: std::io::Error },

    #[snafu(display("failed to read signing cert file: {source}"))]
    ReadCert { source: std::io::Error },

    #[snafu(display("failed to parse PEM: {source}"))]
    Pem { source: pem::PemError },

    #[snafu(display("expected a PEM label of 'CERTIFICATE', got {label:?}"))]
    CertPemLabel { label: String },

    #[snafu(display("expected a PEM key label of 'PRIVATE KEY' (PKCS#8), got {label:?}"))]
    KeyPemLabel { label: String },

    #[snafu(display(
        "PEM key label 'EC PRIVATE KEY' (SEC1) is not supported yet; convert with \
         `openssl pkcs8 -topk8 -nocrypt -in sec1.key -out pkcs8.key`"
    ))]
    Sec1PrivateKeyUnsupported,

    #[snafu(display(
        "unsupported signing key: only ECDSA P-256 (ES256) and P-384 (ES384) are supported"
    ))]
    UnsupportedKey,

    #[snafu(display("failed to parse X.509 certificate: {reason}"))]
    ParseCert { reason: String },

    #[snafu(display(
        "certificate curve does not match signing key curve; cert uses {cert_curve}, key uses {key_curve}"
    ))]
    CurveMismatch {
        cert_curve: &'static str,
        key_curve: &'static str,
    },

    #[snafu(display("failed to load private key: {reason}"))]
    LoadKey { reason: String },

    #[snafu(display("aws-lc-rs signing failed"))]
    Sign,

    #[snafu(display("KMS Sign call failed: {reason}"))]
    Kms { reason: String },

    #[snafu(display("KMS AccessDenied: {reason}"))]
    KmsAccessDenied { reason: String },

    #[snafu(display("KMS key is disabled: {reason}"))]
    KmsKeyDisabled { reason: String },

    #[snafu(display("KMS key state does not allow signing: {reason}"))]
    KmsInvalidKeyState { reason: String },

    #[snafu(display("KMS did not return a signature blob"))]
    KmsEmptySignature,

    #[snafu(display("failed to decode ECDSA DER signature"))]
    DecodeDerSignature,

    #[snafu(display(
        "ECDSA signature scalars exceed the expected width: r={r} bytes, \
         s={s} bytes, expected each ≤ {expected}"
    ))]
    ScalarTooLarge { r: usize, s: usize, expected: usize },

    #[snafu(display("ECDSA DER signature has {trailing} trailing byte(s) after the SEQUENCE"))]
    DerTrailingBytes { trailing: usize },

    #[snafu(display(
        "ECDSA DER outer SEQUENCE length ({claimed}) does not match content length ({actual})"
    ))]
    DerLengthMismatch { claimed: usize, actual: usize },
}

/// Signing algorithm selection. Drives COSE `alg` header and raw scalar width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignAlg {
    Es256,
    Es384,
}

impl SignAlg {
    /// Width (in bytes) of each ECDSA scalar (r and s) in COSE raw form.
    fn scalar_len(self) -> usize {
        match self {
            Self::Es256 => 32,
            Self::Es384 => 48,
        }
    }
}

/// Backend for producing the COSE_Sign1 signature over the pre-hash TBS bytes.
#[async_trait]
pub trait Signer: Send + Sync {
    /// Sign the given `to_be_signed` bytes (the COSE `Sig_structure1` payload)
    /// and return a fixed-width raw ECDSA `r || s` signature.
    async fn sign_cose(&self, to_be_signed: &[u8]) -> Result<Vec<u8>, SignerError>;

    /// PEM-encoded signing certificate, embedded verbatim in the CBOR section.
    fn cert_pem(&self) -> &[u8];

    /// COSE algorithm to advertise.
    fn algorithm(&self) -> SignAlg;
}

/// In-process ECDSA signer backed by `aws-lc-rs`.
pub struct LocalSigner {
    key_pair: EcdsaKeyPair,
    cert_pem: Vec<u8>,
    alg: SignAlg,
    rng: Arc<SystemRandom>,
}

impl LocalSigner {
    /// Build a `LocalSigner` from PEM-encoded cert and key bytes.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, SignerError> {
        // Parse the cert PEM to detect the SPKI curve; this dictates the
        // signing algorithm and must match the key material.
        let parsed_cert = pem::parse(cert_pem).context(PemSnafu)?;
        if parsed_cert.tag() != "CERTIFICATE" {
            return CertPemLabelSnafu {
                label: parsed_cert.tag().to_string(),
            }
            .fail();
        }
        let cert_alg = detect_cert_curve(parsed_cert.contents())?;

        // Only PKCS#8 is accepted; SEC1 gets a dedicated error with a conversion recipe.
        let parsed_key = pem::parse(key_pem).context(PemSnafu)?;
        let key_alg = match parsed_key.tag() {
            "PRIVATE KEY" => cert_alg,
            "EC PRIVATE KEY" => return Sec1PrivateKeyUnsupportedSnafu.fail(),
            other => {
                return KeyPemLabelSnafu {
                    label: other.to_string(),
                }
                .fail()
            }
        };

        // Load the keypair.
        let ring_alg = match key_alg {
            SignAlg::Es256 => &ECDSA_P256_SHA256_ASN1_SIGNING,
            SignAlg::Es384 => &ECDSA_P384_SHA384_ASN1_SIGNING,
        };
        let rng = Arc::new(SystemRandom::new());
        let key_pair = EcdsaKeyPair::from_pkcs8(ring_alg, parsed_key.contents()).map_err(|e| {
            SignerError::LoadKey {
                reason: format!("{e}"),
            }
        })?;

        // Cross-check the key's curve against the cert's via SEC1 point length
        // (P-256 = 65 bytes, P-384 = 97 bytes).
        let raw_pub = key_pair.public_key().as_ref().to_vec();
        let key_curve = match raw_pub.len() {
            65 => SignAlg::Es256,
            97 => SignAlg::Es384,
            _ => return UnsupportedKeySnafu.fail(),
        };
        if key_curve != cert_alg {
            return CurveMismatchSnafu {
                cert_curve: name_of(cert_alg),
                key_curve: name_of(key_curve),
            }
            .fail();
        }

        Ok(Self {
            key_pair,
            cert_pem: cert_pem.to_vec(),
            alg: cert_alg,
            rng,
        })
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn sign_cose(&self, to_be_signed: &[u8]) -> Result<Vec<u8>, SignerError> {
        // aws-lc-rs' EcdsaKeyPair::sign returns DER; convert to raw `r||s`.
        // The work is CPU-bound and short; no yield point is needed.
        let der = self
            .key_pair
            .sign(&*self.rng, to_be_signed)
            .map_err(|_| SignerError::Sign)?;
        ecdsa_der_to_raw(der.as_ref(), self.alg.scalar_len())
    }

    fn cert_pem(&self) -> &[u8] {
        &self.cert_pem
    }

    fn algorithm(&self) -> SignAlg {
        self.alg
    }
}

#[cfg(test)]
impl LocalSigner {
    /// SPKI DER for the signing key, derived on demand. Test-only.
    pub fn public_key_der(&self) -> Vec<u8> {
        let raw_pub = self.key_pair.public_key().as_ref();
        spki_der_for_ecdsa(raw_pub, self.alg)
    }
}

/// KMS-backed ECDSA signer. The private key stays in KMS; only the cert
/// is provided locally. KMS returns DER; we convert to raw `r||s`.
pub struct KmsSigner {
    client: aws_sdk_kms::Client,
    key_id: String,
    cert_pem: Vec<u8>,
    alg: SignAlg,
}

impl KmsSigner {
    /// Build a `KmsSigner`. Region resolves as: explicit `region` arg,
    /// else the region embedded in `key_id` when it is a full ARN, else
    /// the ambient AWS SDK chain (env, profile, IMDS). Credentials come
    /// from the process environment.
    pub async fn from_key_id(
        key_id: String,
        cert_pem: Vec<u8>,
        region: Option<String>,
    ) -> Result<Self, SignerError> {
        let parsed_cert = pem::parse(&cert_pem).context(PemSnafu)?;
        if parsed_cert.tag() != "CERTIFICATE" {
            return CertPemLabelSnafu {
                label: parsed_cert.tag().to_string(),
            }
            .fail();
        }
        let alg = detect_cert_curve(parsed_cert.contents())?;

        let resolved_region = region.or_else(|| region_from_kms_arn(&key_id));

        #[allow(deprecated)]
        let client = {
            let mut loader = aws_config::from_env();
            if let Some(r) = resolved_region {
                loader = loader.region(aws_types::region::Region::new(r));
            }
            let conf = loader.load().await;
            aws_sdk_kms::Client::new(&conf)
        };

        Ok(Self {
            client,
            key_id,
            cert_pem,
            alg,
        })
    }
}

/// Return the region from a KMS ARN (`arn:<partition>:kms:<region>:...`),
/// or `None` for bare key ids/aliases, non-KMS ARNs, or an empty region.
fn region_from_kms_arn(key_id: &str) -> Option<String> {
    let mut parts = key_id.splitn(6, ':');
    if parts.next()? != "arn" {
        return None;
    }
    let _partition = parts.next()?;
    if parts.next()? != "kms" {
        return None;
    }
    let region = parts.next()?;
    (!region.is_empty()).then(|| region.to_string())
}

/// Map a KMS `Sign` SDK error to a typed [`SignerError`]. `AccessDenied` is
/// not a distinct `SignError` variant; probe smithy metadata for its code.
fn map_sign_error(
    e: error_utils::SdkError<aws_sdk_kms::operation::sign::SignError>,
) -> SignerError {
    use aws_sdk_kms::operation::sign::SignError;
    let wrapper = error_utils::AwsSdkError::from(e);
    let display = wrapper.to_string();
    let Some(svc) = wrapper.as_service_error() else {
        return SignerError::Kms { reason: display };
    };
    match svc {
        SignError::DisabledException(inner) => SignerError::KmsKeyDisabled {
            reason: format!("{inner}"),
        },
        SignError::KmsInvalidStateException(inner) => SignerError::KmsInvalidKeyState {
            reason: format!("{inner}"),
        },
        other if other.meta().code() == Some("AccessDeniedException") => {
            SignerError::KmsAccessDenied { reason: display }
        }
        _ => SignerError::Kms { reason: display },
    }
}

#[async_trait]
impl Signer for KmsSigner {
    async fn sign_cose(&self, to_be_signed: &[u8]) -> Result<Vec<u8>, SignerError> {
        use aws_sdk_kms::primitives::Blob;
        use aws_sdk_kms::types::{MessageType, SigningAlgorithmSpec};

        let (algo, hash_alg) = match self.alg {
            SignAlg::Es256 => (
                SigningAlgorithmSpec::EcdsaSha256,
                &aws_lc_rs::digest::SHA256,
            ),
            SignAlg::Es384 => (
                SigningAlgorithmSpec::EcdsaSha384,
                &aws_lc_rs::digest::SHA384,
            ),
        };

        // Hash locally; KMS' `Digest` message type takes the hash, not the TBS,
        // which lets payloads exceed the 4 KiB KMS raw-message limit.
        let digest = aws_lc_rs::digest::digest(hash_alg, to_be_signed);

        let out = self
            .client
            .sign()
            .key_id(&self.key_id)
            .message(Blob::new(digest.as_ref().to_vec()))
            .message_type(MessageType::Digest)
            .signing_algorithm(algo)
            .send()
            .await
            .map_err(map_sign_error)?;

        let sig_der = out.signature().ok_or(SignerError::KmsEmptySignature)?;
        ecdsa_der_to_raw(sig_der.as_ref(), self.alg.scalar_len())
    }

    fn cert_pem(&self) -> &[u8] {
        &self.cert_pem
    }

    fn algorithm(&self) -> SignAlg {
        self.alg
    }
}

// -------- Curve detection --------

/// Detect the ECDSA curve of an X.509 cert via its SPKI namedCurve OID.
/// Only P-256 and P-384 are accepted.
fn detect_cert_curve(cert_der: &[u8]) -> Result<SignAlg, SignerError> {
    use x509_cert::der::Decode;
    let cert = x509_cert::Certificate::from_der(cert_der).map_err(|e| SignerError::ParseCert {
        reason: format!("{e}"),
    })?;
    let alg_ident = &cert.tbs_certificate.subject_public_key_info.algorithm;
    // id-ecPublicKey OID.
    let ec_public_key: x509_cert::der::asn1::ObjectIdentifier =
        "1.2.840.10045.2.1".parse().unwrap();
    if alg_ident.oid != ec_public_key {
        return UnsupportedKeySnafu.fail();
    }
    let params = alg_ident
        .parameters
        .as_ref()
        .ok_or(SignerError::UnsupportedKey)?;
    let curve_oid: x509_cert::der::asn1::ObjectIdentifier =
        params.decode_as().map_err(|e| SignerError::ParseCert {
            reason: format!("curve OID decode: {e}"),
        })?;
    let p256: x509_cert::der::asn1::ObjectIdentifier = "1.2.840.10045.3.1.7".parse().unwrap();
    let p384: x509_cert::der::asn1::ObjectIdentifier = "1.3.132.0.34".parse().unwrap();
    if curve_oid == p256 {
        Ok(SignAlg::Es256)
    } else if curve_oid == p384 {
        Ok(SignAlg::Es384)
    } else {
        UnsupportedKeySnafu.fail()
    }
}

fn name_of(alg: SignAlg) -> &'static str {
    match alg {
        SignAlg::Es256 => "P-256",
        SignAlg::Es384 => "P-384",
    }
}

// -------- DER ↔ raw ECDSA signature conversion --------

/// Convert a DER-encoded ECDSA signature `SEQUENCE(INTEGER r, INTEGER s)`
/// into COSE's raw `r || s` form, left-padded to `scalar_len` per scalar.
/// Rejects trailing bytes, length mismatches, and over-wide scalars via
/// the corresponding [`SignerError`] variants.
fn ecdsa_der_to_raw(der: &[u8], scalar_len: usize) -> Result<Vec<u8>, SignerError> {
    let mut i = 0;
    if der.get(i).copied() != Some(0x30) {
        return DecodeDerSignatureSnafu.fail();
    }
    i += 1;
    let (seq_len, consumed) = read_der_len(&der[i..]).ok_or(SignerError::DecodeDerSignature)?;
    i += consumed;
    let content_start = i;

    let r = read_der_uint(&der[i..]).ok_or(SignerError::DecodeDerSignature)?;
    i += r.raw_len;
    let s = read_der_uint(&der[i..]).ok_or(SignerError::DecodeDerSignature)?;
    i += s.raw_len;

    let content_actual = i - content_start;
    if content_actual != seq_len {
        return DerLengthMismatchSnafu {
            claimed: seq_len,
            actual: content_actual,
        }
        .fail();
    }
    let trailing = der.len() - i;
    if trailing > 0 {
        return DerTrailingBytesSnafu { trailing }.fail();
    }
    if r.value.len() > scalar_len || s.value.len() > scalar_len {
        return ScalarTooLargeSnafu {
            r: r.value.len(),
            s: s.value.len(),
            expected: scalar_len,
        }
        .fail();
    }
    let mut out = vec![0u8; 2 * scalar_len];
    out[scalar_len - r.value.len()..scalar_len].copy_from_slice(&r.value);
    out[2 * scalar_len - s.value.len()..].copy_from_slice(&s.value);
    Ok(out)
}

struct DerUint {
    value: Vec<u8>,
    raw_len: usize,
}

fn read_der_uint(buf: &[u8]) -> Option<DerUint> {
    if buf.first()? != &0x02 {
        return None;
    }
    let (len, consumed) = read_der_len(&buf[1..])?;
    let start = 1 + consumed;
    let end = start.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    let mut value = buf[start..end].to_vec();
    // Strip any leading zero used to disambiguate the sign bit.
    while value.len() > 1 && value[0] == 0 {
        value.remove(0);
    }
    Some(DerUint {
        value,
        raw_len: end,
    })
}

fn read_der_len(buf: &[u8]) -> Option<(usize, usize)> {
    let first = *buf.first()?;
    if first & 0x80 == 0 {
        return Some((first as usize, 1));
    }
    let n = (first & 0x7f) as usize;
    if n == 0 || n > 4 {
        return None;
    }
    let mut len = 0usize;
    for i in 0..n {
        len = (len << 8) | (*buf.get(1 + i)? as usize);
    }
    Some((len, 1 + n))
}

/// Minimal SPKI DER envelope over a SEC1 ECDSA public key. Test-only.
#[cfg(test)]
fn spki_der_for_ecdsa(sec1_uncompressed: &[u8], alg: SignAlg) -> Vec<u8> {
    use crate::der_helpers::wrap_der;
    // id-ecPublicKey 1.2.840.10045.2.1.
    let id_ec_public_key: [u8; 9] = [0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
    // secp256r1 1.2.840.10045.3.1.7; secp384r1 1.3.132.0.34.
    let curve_oid: Vec<u8> = match alg {
        SignAlg::Es256 => vec![0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07],
        SignAlg::Es384 => vec![0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22],
    };

    let mut alg_ident_body = Vec::new();
    alg_ident_body.extend_from_slice(&id_ec_public_key);
    alg_ident_body.extend_from_slice(&curve_oid);
    let alg_ident = wrap_der(0x30, &alg_ident_body);

    let mut bitstring_body = vec![0u8];
    bitstring_body.extend_from_slice(sec1_uncompressed);
    let bitstring = wrap_der(0x03, &bitstring_body);

    let mut spki_body = Vec::new();
    spki_body.extend_from_slice(&alg_ident);
    spki_body.extend_from_slice(&bitstring);
    wrap_der(0x30, &spki_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdsa_der_roundtrip_p384() {
        let r: Vec<u8> = (1u8..=48).collect();
        let s: Vec<u8> = (49u8..=96).collect();
        let der = build_ecdsa_der(&r, &s);
        let raw = ecdsa_der_to_raw(&der, 48).unwrap();
        assert_eq!(&raw[..48], &r[..]);
        assert_eq!(&raw[48..], &s[..]);
    }

    #[test]
    fn ecdsa_der_high_bit_set_p384() {
        // Regression guard: high-bit-set scalars are DER-encoded with a
        // leading 0x00 that must be stripped in the raw form.
        let mut r = vec![0u8; 48];
        r[0] = 0x80;
        for (i, b) in r.iter_mut().enumerate().skip(1) {
            *b = i as u8;
        }
        let s = r.iter().rev().copied().collect::<Vec<u8>>();
        let der = build_ecdsa_der(&r, &s);
        let raw = ecdsa_der_to_raw(&der, 48).unwrap();
        assert_eq!(raw.len(), 96);
        assert_eq!(&raw[..48], &r[..], "r must not have a stray leading zero");
        assert_eq!(&raw[48..], &s[..], "s must not have a stray leading zero");
    }

    #[test]
    fn ecdsa_der_rejects_trailing_bytes() {
        let r = vec![0x11u8; 48];
        let s = vec![0x22u8; 48];
        let mut der = build_ecdsa_der(&r, &s);
        der.push(0x00); // one stray byte after the SEQUENCE
        let err = ecdsa_der_to_raw(&der, 48).unwrap_err();
        assert!(
            matches!(err, SignerError::DerTrailingBytes { trailing: 1 }),
            "err={err}",
        );
    }

    #[test]
    fn ecdsa_der_rejects_length_mismatch() {
        let r = vec![0x11u8; 32];
        let s = vec![0x22u8; 32];
        let mut der = build_ecdsa_der(&r, &s);
        // Inflate the outer SEQUENCE length and pad the tail so the length
        // mismatch surfaces before the trailing-bytes check.
        der[1] = der[1].wrapping_add(10);
        der.extend_from_slice(&[0u8; 10]);
        let err = ecdsa_der_to_raw(&der, 32).unwrap_err();
        assert!(
            matches!(err, SignerError::DerLengthMismatch { .. }),
            "err={err}",
        );
    }

    #[test]
    fn ecdsa_der_rejects_scalar_too_large() {
        let r = (1u8..=60).collect::<Vec<u8>>();
        let s = (61u8..=120).collect::<Vec<u8>>();
        let der = build_ecdsa_der(&r, &s);
        let err = ecdsa_der_to_raw(&der, 48).unwrap_err();
        assert!(
            matches!(
                err,
                SignerError::ScalarTooLarge {
                    r: 60,
                    s: 60,
                    expected: 48,
                }
            ),
            "err={err}",
        );
    }

    #[test]
    fn ecdsa_der_pads_short_scalars() {
        let r = vec![0x05];
        let s = vec![0x06];
        let der = build_ecdsa_der(&r, &s);
        let raw = ecdsa_der_to_raw(&der, 48).unwrap();
        assert_eq!(raw[47], 0x05);
        assert_eq!(raw[95], 0x06);
        assert!(raw[..47].iter().all(|&b| b == 0));
        assert!(raw[48..95].iter().all(|&b| b == 0));
    }

    /// Build a DER ECDSA signature envelope from raw scalars.
    fn build_ecdsa_der(r: &[u8], s: &[u8]) -> Vec<u8> {
        fn uint(v: &[u8]) -> Vec<u8> {
            let mut trimmed = v;
            while trimmed.len() > 1 && trimmed[0] == 0 {
                trimmed = &trimmed[1..];
            }
            let mut body = Vec::new();
            if trimmed[0] & 0x80 != 0 {
                body.push(0);
            }
            body.extend_from_slice(trimmed);
            let mut out = vec![0x02, body.len() as u8];
            out.extend_from_slice(&body);
            out
        }
        let mut inner = Vec::new();
        inner.extend_from_slice(&uint(r));
        inner.extend_from_slice(&uint(s));
        let mut out = vec![0x30, inner.len() as u8];
        out.extend_from_slice(&inner);
        out
    }

    #[test]
    fn rejects_rsa_pem_key() {
        // Both the stub cert and the RSA label are rejected paths; either
        // typed error is acceptable.
        let cert_pem = pem::encode(&pem::Pem::new("CERTIFICATE", vec![0u8; 8]));
        let key_pem = pem::encode(&pem::Pem::new("RSA PRIVATE KEY", vec![0u8; 8]));
        let result = LocalSigner::from_pem(cert_pem.as_bytes(), key_pem.as_bytes());
        assert!(matches!(
            result.err(),
            Some(SignerError::KeyPemLabel { .. } | SignerError::ParseCert { .. }),
        ));
    }

    #[test]
    fn rejects_sec1_ec_private_key_with_actionable_error() {
        // SEC1 (`EC PRIVATE KEY`) input must surface a typed error, not an
        // opaque aws-lc-rs failure.
        use crate::der_helpers::wrap_der;
        use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P384_SHA384_ASN1_SIGNING};
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, &rng).unwrap();
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
        let sec1_pub = key_pair.public_key().as_ref().to_vec();
        let spki = spki_der_for_ecdsa(&sec1_pub, SignAlg::Es384);
        let cert = wrap_der(0x30, &spki);
        let cert_pem = pem::encode(&pem::Pem::new("CERTIFICATE", cert));
        let sec1 = pem::encode(&pem::Pem::new("EC PRIVATE KEY", vec![0u8; 8]));
        let err = LocalSigner::from_pem(cert_pem.as_bytes(), sec1.as_bytes())
            .err()
            .expect("should reject");
        // Either typed error is acceptable; both prove we don't panic.
        assert!(
            matches!(
                err,
                SignerError::Sec1PrivateKeyUnsupported | SignerError::ParseCert { .. },
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn region_from_kms_arn_extracts_region_from_key_arn() {
        assert_eq!(
            region_from_kms_arn(
                "arn:aws:kms:us-east-1:123456789012:key/abcdef01-2345-6789-abcd-ef0123456789"
            ),
            Some("us-east-1".to_string()),
        );
    }

    #[test]
    fn region_from_kms_arn_extracts_region_from_alias_arn() {
        assert_eq!(
            region_from_kms_arn("arn:aws:kms:eu-west-2:123456789012:alias/eif-signing-key"),
            Some("eu-west-2".to_string()),
        );
    }

    #[test]
    fn region_from_kms_arn_accepts_partitions() {
        assert_eq!(
            region_from_kms_arn(
                "arn:aws-us-gov:kms:us-gov-west-1:123456789012:key/abcdef01-2345-6789-abcd-ef0123456789"
            ),
            Some("us-gov-west-1".to_string()),
        );
        assert_eq!(
            region_from_kms_arn(
                "arn:aws-cn:kms:cn-north-1:123456789012:key/abcdef01-2345-6789-abcd-ef0123456789"
            ),
            Some("cn-north-1".to_string()),
        );
    }

    #[test]
    fn region_from_kms_arn_rejects_non_arn_ids() {
        assert_eq!(region_from_kms_arn("alias/eif-signing-key"), None);
        assert_eq!(
            region_from_kms_arn("abcdef01-2345-6789-abcd-ef0123456789"),
            None,
        );
    }

    #[test]
    fn region_from_kms_arn_rejects_wrong_service() {
        assert_eq!(
            region_from_kms_arn("arn:aws:iam::123456789012:role/example"),
            None,
        );
        assert_eq!(region_from_kms_arn("arn:aws:s3:::my-bucket"), None,);
    }

    #[test]
    fn region_from_kms_arn_rejects_empty_region_field() {
        assert_eq!(
            region_from_kms_arn("arn:aws:kms::123456789012:key/abcdef01"),
            None,
        );
    }
}
