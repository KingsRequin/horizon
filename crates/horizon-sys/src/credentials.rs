//! Secrets dans le Gestionnaire d'identifiants de Windows (identifiants génériques, propres à la
//! session) : jetons et mots de passe des réglages des plugins, jamais écrits dans un fichier
//! d'Horizon.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{HSTRING, PWSTR};

/// Taille maximale d'un secret (limite de Windows : 2 560 octets).
pub const MAX_SECRET: usize = 2560;

/// Enregistre `secret` sous le nom `target` (remplace l'ancien).
pub fn write(target: &str, secret: &str) -> anyhow::Result<()> {
    anyhow::ensure!(secret.len() <= MAX_SECRET, "secret trop long ({MAX_SECRET} octets au plus)");
    let mut name: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut blob = secret.as_bytes().to_vec();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        Comment: PWSTR::null(),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR::null(),
    };
    // SAFETY : structure remplie de tampons vivants jusqu'au retour de l'appel.
    unsafe { CredWriteW(&cred, 0)? };
    Ok(())
}

/// Secret enregistré sous `target` (`None` : aucun).
pub fn read(target: &str) -> Option<String> {
    let mut out: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY : `out` reçoit une structure allouée par Windows, libérée par `CredFree`.
    unsafe {
        CredReadW(&HSTRING::from(target), CRED_TYPE_GENERIC, None, &mut out).ok()?;
        let c = &*out;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(out as *const _);
        String::from_utf8(bytes).ok()
    }
}

/// Efface le secret `target` (rien s'il n'existe pas).
pub fn delete(target: &str) {
    // SAFETY : nom valide le temps de l'appel.
    let _ = unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_roundtrip() {
        let target = format!("Horizon/essai/{}", std::process::id());
        if write(&target, "jeton secret é").is_err() {
            // Machines de CI sans magasin d'identifiants : rien à vérifier.
            assert!(std::env::var_os("CI").is_some(), "magasin d'identifiants indisponible sur un poste de travail");
            return;
        }
        assert_eq!(read(&target).as_deref(), Some("jeton secret é"));
        write(&target, "autre").unwrap();
        assert_eq!(read(&target).as_deref(), Some("autre"));
        delete(&target);
        assert_eq!(read(&target), None);
    }
}
