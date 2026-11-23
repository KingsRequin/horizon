//! Identité TLS du PC : une clé créée une fois, un certificat auto-signé refait à chaque
//! lancement. Le téléphone épingle l'empreinte de la clé publique, pas celle du certificat.

use anyhow::{Context, anyhow};
use base64::Engine;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::sync::Arc;

/// La clé du PC et ce qui en découle pour une session TLS.
pub struct Identity {
    fingerprint: String,
    tls: Arc<rustls::ServerConfig>,
}

impl Identity {
    /// Clé lue par `read` (PEM PKCS#8) ; absente ou illisible, elle est créée et donnée à
    /// `write`. Le certificat, lui, n'est jamais gardé.
    pub fn load_or_create(read: impl Fn() -> Option<String>, write: impl Fn(&str)) -> anyhow::Result<Identity> {
        let key = match read().and_then(|pem| KeyPair::from_pem(&pem).ok()) {
            Some(key) => key,
            None => {
                let key = KeyPair::generate().context("création de la clé du PC")?;
                write(&key.serialize_pem());
                key
            }
        };
        let mut params = CertificateParams::new(vec!["horizon.local".to_owned()]).context("certificat du PC")?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "Horizon");
        let cert = params.self_signed(&key).context("signature du certificat du PC")?.der().clone();
        let fingerprint = spki_fingerprint(&cert)?;
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .context("versions de TLS")?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key_der)
            .context("configuration TLS du PC")?;
        Ok(Identity { fingerprint, tls: Arc::new(tls) })
    }

    /// Empreinte de la clé publique, celle du QR code d'appairage.
    pub fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    pub fn tls_config(&self) -> Arc<rustls::ServerConfig> {
        self.tls.clone()
    }
}

/// Empreinte de la clé publique d'un certificat : SHA-256 du SPKI, base64url sans remplissage
/// (`docs/PHONE.md`).
fn spki_fingerprint(cert: &CertificateDer<'_>) -> anyhow::Result<String> {
    let parsed = webpki::EndEntityCert::try_from(cert).map_err(|e| anyhow!("certificat du PC illisible : {e}"))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, parsed.subject_public_key_info().as_ref());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

/// Jeton aléatoire de `bytes` octets, en base64url sans remplissage.
pub fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut buffer).expect("générateur aléatoire du système");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn the_fingerprint_is_stable_across_restarts() {
        let store = Mutex::new(None::<String>);
        let a = Identity::load_or_create(|| store.lock().unwrap().clone(), |pem| *store.lock().unwrap() = Some(pem.to_owned())).unwrap();
        let b = Identity::load_or_create(|| store.lock().unwrap().clone(), |_| panic!("clé déjà là")).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint().len(), 43);
        assert!(a.fingerprint().chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "base64url sans remplissage");
    }

    #[test]
    fn an_unreadable_key_is_replaced() {
        let written = Mutex::new(None::<String>);
        let id = Identity::load_or_create(|| Some("pas une clé".to_owned()), |pem| *written.lock().unwrap() = Some(pem.to_owned())).unwrap();
        assert_eq!(id.fingerprint().len(), 43);
        assert!(written.lock().unwrap().as_deref().is_some_and(|pem| pem.contains("PRIVATE KEY")));
    }

    #[test]
    fn tokens_are_random_and_url_safe() {
        let (a, b) = (random_token(16), random_token(16));
        assert_ne!(a, b);
        assert_eq!(a.len(), 22);
        assert_eq!(random_token(32).len(), 43);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
