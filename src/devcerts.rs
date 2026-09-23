use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use core::pin::Pin;
use cxx_qt_lib::{QString, QStringList};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use crate::common::{
    Family, distro_id, family, family_str, log_debug, privileged_output, to_qstringlist,
    valid_cert_domain,
};

pub(crate) fn certs_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join("certs")
}

fn ca_key_path() -> PathBuf {
    certs_dir().join("myCA.key")
}

fn ca_pem_path() -> PathBuf {
    certs_dir().join("myCA.pem")
}

#[allow(dead_code)]
fn ca_srl_path() -> PathBuf {
    certs_dir().join("myCA.srl")
}

fn passphrase_path() -> PathBuf {
    certs_dir().join(".ca_passphrase")
}

#[allow(dead_code)]
fn installed_crt_path() -> PathBuf {
    installed_crt_path_for(&family())
}

fn installed_crt_path_for(family: &Family) -> PathBuf {
    match family {
        Family::Fedora => PathBuf::from("/etc/pki/ca-trust/source/anchors/myCA.crt"),
        Family::Debian => PathBuf::from("/usr/local/share/ca-certificates/myCA.crt"),
        Family::Unsupported => PathBuf::from(""),
    }
}

fn update_command_for(family: &Family) -> &'static str {
    match family {
        Family::Fedora => "update-ca-trust",
        Family::Debian => "update-ca-certificates",
        Family::Unsupported => "",
    }
}

fn devbox_state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/state/devbox.arc")
}

fn machine_id_bytes() -> Vec<u8> {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = fs::read_to_string(p) {
            let t = s.trim().to_string();
            if !t.is_empty() {
                return t.into_bytes();
            }
        }
    }
    b"ducknet-dev-tool-default-key".to_vec()
}

fn xor_cipher(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter().enumerate().map(|(i, b)| b ^ key[i % key.len()]).collect()
}

fn encrypt_and_store_passphrase(passphrase: &str) -> Result<(), String> {
    let key = machine_id_bytes();
    let enc = xor_cipher(passphrase.as_bytes(), &key);
    let b64 = BASE64.encode(&enc);
    let path = devbox_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    fs::write(&path, &b64).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    log_debug(&format!("[ducknet] encrypted passphrase stored to {} ({} bytes)", path.display(), b64.len()));
    Ok(())
}

fn decrypt_stored_passphrase() -> Result<String, String> {
    let path = devbox_state_path();
    let b64 = fs::read_to_string(&path).map_err(|e| format!("Failed to read {}: {} — did you run Setup CA?", path.display(), e))?;
    let b64 = b64.trim();
    if b64.is_empty() {
        return Err("Stored passphrase is empty".to_string());
    }
    let enc = BASE64.decode(b64).map_err(|e| format!("Failed to base64 decode {}: {}", path.display(), e))?;
    let key = machine_id_bytes();
    let dec = xor_cipher(&enc, &key);
    String::from_utf8(dec).map_err(|e| format!("Failed to decrypt passphrase: {}", e))
}

fn ensure_nss_db(nssdb: &PathBuf) -> Result<(), String> {
    if nssdb.join("cert9.db").exists() || nssdb.join("cert8.db").exists() {
        return Ok(());
    }
    fs::create_dir_all(nssdb).map_err(|e| format!("Failed to create {}: {}", nssdb.display(), e))?;
    // Init missing NSS DB with empty password.
    let db_str = format!("sql:{}", nssdb.display());
    let out = Command::new("certutil")
        .args(["-N", "-d", &db_str, "--empty-password"])
        .output()
        .map_err(|e| format!("Failed to run certutil -N: {} — is nss-tools installed?", e))?;
    if !out.status.success() {
        // Fallback to legacy dbm format.
        let out2 = Command::new("certutil")
            .args(["-N", "-d", &nssdb.to_string_lossy(), "--empty-password"])
            .output();
        if let Ok(o2) = out2 {
            if o2.status.success() {
                return Ok(());
            }
        }
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("certutil -N failed: {}", err.trim()));
    }
    Ok(())
}

/// Firefox profile base dirs: Debian uses ~/.mozilla/firefox,
/// Fedora uses ~/.config/mozilla/firefox, Flatpak uses
/// ~/.var/app/org.mozilla.firefox/.mozilla/firefox.
fn firefox_base_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    vec![
        PathBuf::from(&home).join(".mozilla/firefox"),
        PathBuf::from(&home).join(".config/mozilla/firefox"),
        PathBuf::from(&home).join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
    ]
}

/// Profiles from profiles.ini: Default=1, [Install] defaults, then first.
/// [Install] defaults cover ESR (no Default=1 marker).
/// Returns only existing dirs.
fn profiles_from_ini(base: &PathBuf) -> Vec<PathBuf> {
    let ini = match fs::read_to_string(base.join("profiles.ini")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut cur_path: Option<String> = None;
    let mut cur_relative = true;
    let mut cur_default = false;
    let mut install_defaults: Vec<String> = Vec::new();
    let mut default_path: Option<(String, bool)> = None;
    let mut first_path: Option<(String, bool)> = None;
    macro_rules! flush_section {
        () => {
            if let Some((p, r)) = cur_path.take().map(|p| (p, cur_relative)) {
                if first_path.is_none() {
                    first_path = Some((p.clone(), r));
                }
                if cur_default {
                    default_path = Some((p, r));
                }
            }
        };
    }
    for line in ini.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            flush_section!();
            cur_path = None;
            cur_relative = true;
            cur_default = false;
        } else if let Some(v) = t.strip_prefix("Path=") {
            cur_path = Some(v.trim().to_string());
        } else if let Some(v) = t.strip_prefix("IsRelative=") {
            cur_relative = v.trim() != "0";
        } else if let Some(v) = t.strip_prefix("Default=") {
            let v = v.trim();
            if v == "1" {
                cur_default = true;
            } else if !v.is_empty() {
                // [Install] Default=<dir>, relative to base.
                install_defaults.push(v.to_string());
            }
        }
    }
    flush_section!();
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |rel: &str, is_rel: bool| {
        let full = if is_rel { base.join(rel) } else { PathBuf::from(rel) };
        if full.is_dir() && !out.contains(&full) {
            log_debug(&format!("[ducknet] firefox profile from {}.profiles.ini: {}", base.display(), full.display()));
            out.push(full);
        }
    };
    if let Some((rel, is_rel)) = default_path {
        push(&rel, is_rel);
    }
    for rel in &install_defaults {
        push(rel, true);
    }
    if let Some((rel, is_rel)) = first_path {
        push(&rel, is_rel);
    }
    out
}

/// All managed Firefox profiles: profiles.ini entries plus *.default* dirs.
/// Upgrades strand trust across profiles, so touch all — never one.
fn find_firefox_profiles() -> Vec<PathBuf> {
    let mut profiles: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() {
            let key = p.to_string_lossy().to_string();
            if !seen.contains(&key) {
                seen.push(key);
                profiles.push(p);
            }
        }
    };
    // 1. profiles.ini: Default=1, [Install] defaults (incl. ESR), first profile.
    for base in firefox_base_dirs() {
        for p in profiles_from_ini(&base) {
            push(p);
        }
    }
    // 2. every *.default-release / *.default / *.default-esr dir across all bases
    let mut scanned: Vec<PathBuf> = Vec::new();
    for base in firefox_base_dirs() {
        if let Ok(entries) = fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                        if name.ends_with(".default-release")
                            || name.ends_with(".default-esr")
                            || name.ends_with(".default")
                        {
                            scanned.push(p);
                        }
                    }
                }
            }
        }
    }
    // Deterministic order: .default-release first, then path sort.
    scanned.sort_by(|a, b| {
        let a_is_release = a.to_string_lossy().contains(".default-release");
        let b_is_release = b.to_string_lossy().contains(".default-release");
        b_is_release.cmp(&a_is_release).then_with(|| a.cmp(b))
    });
    for p in scanned {
        push(p);
    }
    profiles
}

/// Remove a domain's ~/certs files (.key/.csr/.crt/.ext). Returns (files_removed, errors).
fn delete_domain_files(domain: &str) -> (usize, Vec<String>) {
    let dir = certs_dir();
    let to_remove = [
        dir.join(format!("{}.key", domain)),
        dir.join(format!("{}.csr", domain)),
        dir.join(format!("{}.crt", domain)),
        dir.join(format!("{}.ext", domain)),
    ];
    let mut removed = 0;
    let mut errors = Vec::new();
    for p in &to_remove {
        if p.exists() {
            match fs::remove_file(p) {
                Ok(_) => {
                    log_debug(&format!("[ducknet] deleteCert removed {}", p.display()));
                    removed += 1;
                }
                Err(e) => {
                    let msg = format!("Failed to remove {}: {}", p.display(), e);
                    log_debug(&format!("[ducknet] {}", msg));
                    errors.push(msg);
                }
            }
        }
    }
    (removed, errors)
}

/// Remove a domain's /etc/hosts entry (127.0.0.1 and legacy 12.0.0.1). Best effort.
fn remove_hosts_entry(domain: &str) {
    let hosts_content = fs::read_to_string("/etc/hosts").unwrap_or_default();
    let has_entry = hosts_content.lines().any(|l| {
        let t = l.trim();
        if t.starts_with('#') || t.is_empty() { return false; }
        t.split_whitespace().any(|w| w == domain)
    });
    if !has_entry {
        return;
    }
    // Pass domain as $1 to avoid shell injection.
    match privileged_output(&["sh", "-c", "sed -i \"/[[:space:]]$1[[:space:]]*$/d\" /etc/hosts", "sh", domain]) {
        Ok(o) if o.status.success() => log_debug(&format!("[ducknet] removed {} from /etc/hosts", domain)),
        Ok(o) => log_debug(&format!("[ducknet] failed to remove {} from /etc/hosts (exit {}): {}{}", domain, o.status, String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout))),
        Err(e) => log_debug(&format!("[ducknet] hosts remove failed: {}", e)),
    }
    // Fallback clears legacy IP forms.
    let _ = privileged_output(&["sh", "-c", "sed -i \"/^127\\.0\\.0\\.1[[:space:]]\\+$1[[:space:]]*$/d\" /etc/hosts; sed -i \"/^12\\.0\\.0\\.1[[:space:]]\\+$1[[:space:]]*$/d\" /etc/hosts", "sh", domain]);
}

/// Remove "My Local Development CA" from ALL managed Firefox profiles' NSSDBs.
/// Ok(user_message) when every targeted profile is clean or was already
/// absent; Err(msg) listing the profiles that still hold the CA.
fn remove_firefox_ca() -> Result<String, String> {
    let profiles = find_firefox_profiles();
    if profiles.is_empty() {
        return Err("No Firefox profile found at ~/.mozilla/firefox or ~/.config/mozilla/firefox (*.default-release / *.default) — launch Firefox once to create a profile".to_string());
    }
    let mut cleaned: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for profile in &profiles {
        let label = profile.file_name().and_then(|s| s.to_str()).unwrap_or("profile").to_string();
        let db_arg = format!("sql:{}", profile.display());
        let db_raw = profile.to_string_lossy().to_string();
        let mut done = false;
        for db in [&db_arg, &db_raw] {
            match Command::new("certutil").args(["-D", "-d", db, "-n", "My Local Development CA"]).output() {
                Ok(o) if o.status.success() => {
                    log_debug(&format!("[ducknet] firefox delete succeeded in {} with {}", label, db));
                    cleaned.push(label.clone());
                    done = true;
                    break;
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    log_debug(&format!("[ducknet] firefox delete in {} with {} failed: {}", label, db, err));
                }
                Err(e) => return Err(format!("Failed to run certutil: {} — is nss-tools installed?", e)),
            }
        }
        if done {
            continue;
        }
        // Absent now means already removed.
        if let Ok(o) = Command::new("certutil").args(["-L", "-d", &db_arg, "-n", "My Local Development CA"]).output() {
            if !o.status.success() {
                log_debug(&format!("[ducknet] firefox CA already absent in {}", label));
                continue;
            }
        }
        failed.push(label);
    }
    if !failed.is_empty() {
        return Err(format!("Failed to remove CA from Firefox profile(s): {}", failed.join(", ")));
    }
    if cleaned.is_empty() {
        return Ok("CA not found in Firefox — already removed".to_string());
    }
    Ok(format!("Removed CA from Firefox ({})", cleaned.join(", ")))
}

/// True when "My Local Development CA" is present in ANY managed Firefox
/// profile NSSDB. `certutil -L -d <db> -n ...` exit 0 means installed.
fn is_firefox_ca_installed() -> bool {
    let profiles = find_firefox_profiles();
    if profiles.is_empty() {
        return false;
    }
    for profile in &profiles {
        let db_arg = format!("sql:{}", profile.display());
        let db_raw = profile.to_string_lossy().to_string();
        for db in [&db_arg, &db_raw] {
            match Command::new("certutil")
                .args(["-L", "-d", db, "-n", "My Local Development CA"])
                .output()
            {
                Ok(o) if o.status.success() => {
                    log_debug(&format!("[ducknet] firefox CA detected in {}", profile.display()));
                    return true;
                }
                Ok(o) => {
                    log_debug(&format!(
                        "[ducknet] firefox CA not in {} (exit {}): {}",
                        db,
                        o.status,
                        String::from_utf8_lossy(&o.stderr).trim()
                    ));
                }
                Err(e) => {
                    log_debug(&format!("[ducknet] certutil launch failed: {} — is nss-tools installed?", e));
                    return false;
                }
            }
        }
    }
    false
}

fn ensure_bundle_p12(decrypted_pass: &str) -> Option<PathBuf> {
    let dir = certs_dir();
    let bundle = dir.join("bundle.p12");
    if bundle.exists() {
        return Some(bundle);
    }
    // Generate bundle.p12 only when missing.
    let key = ca_key_path();
    let pem = ca_pem_path();
    let pp_path = passphrase_path();
    if !key.exists() || !pem.exists() {
        return None;
    }
    // -passin reads file to keep passphrase out of process args.
    let out = Command::new("openssl")
        .args([
            "pkcs12",
            "-export",
            "-out",
            &bundle.to_string_lossy(),
            "-inkey",
            &key.to_string_lossy(),
            "-in",
            &pem.to_string_lossy(),
            "-passin",
            &format!("file:{}", pp_path.to_string_lossy()),
            "-passout",
            &format!("pass:{}", decrypted_pass),
            "-name",
            "My Local Development CA",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() && bundle.exists() => {
            let _ = fs::set_permissions(&bundle, fs::Permissions::from_mode(0o600));
            log_debug(&format!("[ducknet] generated bundle.p12 at {}", bundle.display()));
            Some(bundle)
        }
        Ok(o) => {
            log_debug(&format!("[ducknet] openssl pkcs12 failed (bundle not created): {} {}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout)));
            None
        }
        Err(e) => {
            log_debug(&format!("[ducknet] openssl pkcs12 launch failed: {}", e));
            None
        }
    }
}

pub fn detect_ca_status() -> String {
    let family = family();
    if family == Family::Unsupported {
        return "Unsupported".to_string();
    }
    let path = installed_crt_path_for(&family);
    if path.exists() {
        "Installed".to_string()
    } else {
        "NotSetup".to_string()
    }
}

fn detect_common_name() -> String {
    let pem = ca_pem_path();
    if !pem.exists() {
        return String::new();
    }
    let out = Command::new("openssl")
        .args(["x509", "-noout", "-subject", "-in"])
        .arg(&pem)
        .output();
    if let Ok(output) = out {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).to_string();
            // s like "subject=C = US, ST = ..., CN = myCA"
            if let Some(idx) = s.find("CN = ") {
                let rest = &s[idx + 5..];
                let end = rest.find(',').unwrap_or_else(|| rest.find('\n').unwrap_or(rest.len()));
                return rest[..end].trim().to_string();
            }
            if let Some(idx) = s.find("CN=") {
                let rest = &s[idx + 3..];
                let end = rest.find(',').unwrap_or_else(|| rest.find('/').unwrap_or(rest.len()));
                return rest[..end].trim().to_string();
            }
        }
    }
    String::new()
}

pub(crate) fn list_generated_certs() -> Vec<String> {
    let dir = certs_dir();
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("crt") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem != "myCA" {
                        result.push(stem.to_string());
                    }
                }
            }
        }
    }
    result.sort();
    result
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, ca_setup_status, cxx_name = "caSetupStatus")]
        #[qproperty(QString, common_name, cxx_name = "commonName")]
        #[qproperty(QStringList, generated_certs, cxx_name = "generatedCerts")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        #[qproperty(QString, distro_family, cxx_name = "distroFamily")]
        #[qproperty(QString, distro_id, cxx_name = "distroId")]
        #[qproperty(QString, installed_cert_path, cxx_name = "installedCertPath")]
        #[qproperty(QString, update_command, cxx_name = "updateCommand")]
        #[qproperty(bool, firefox_ca_installed, cxx_name = "firefoxCaInstalled")]
        type DevCertsManager = super::DevCertsManagerRust;

        #[qsignal]
        #[cxx_name = "statusChanged"]
        fn status_changed(self: Pin<&mut Self>, message: QString);

        #[qsignal]
        #[cxx_name = "progressUpdated"]
        fn progress_updated(self: Pin<&mut Self>, message: QString);

        #[qinvokable]
        #[cxx_name = "setupCA"]
        fn setup_ca(
            self: Pin<&mut Self>,
            passphrase: &QString,
            country: &QString,
            state: &QString,
            locality: &QString,
            organization: &QString,
            org_unit: &QString,
            common_name: &QString,
            email: &QString,
            validity_days: i32,
            key_size: i32,
        ) -> bool;

        #[qinvokable]
        #[cxx_name = "installRootCert"]
        fn install_root_cert(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "removeRootCert"]
        fn remove_root_cert(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "generateCert"]
        fn generate_cert(
            self: Pin<&mut Self>,
            domain: &QString,
            sans_list: &QStringList,
            wildcard: bool,
            validity_days: i32,
        ) -> bool;

        #[qinvokable]
        #[cxx_name = "deleteCert"]
        fn delete_cert(self: Pin<&mut Self>, domain: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "installToChrome"]
        fn install_to_chrome(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "installToFirefox"]
        fn install_to_firefox(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "deleteFromChrome"]
        fn delete_from_chrome(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "deleteFromFirefox"]
        fn delete_from_firefox(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "refreshFirefoxStatus"]
        fn refresh_firefox_status(self: Pin<&mut Self>) -> bool;
    }
}

pub struct DevCertsManagerRust {
    ca_setup_status: QString,
    common_name: QString,
    generated_certs: QStringList,
    status_message: QString,
    distro_family: QString,
    distro_id: QString,
    installed_cert_path: QString,
    update_command: QString,
    firefox_ca_installed: bool,
}

impl qobject::DevCertsManager {
    fn setup_ca(
        mut self: Pin<&mut Self>,
        passphrase: &QString,
        country: &QString,
        state: &QString,
        locality: &QString,
        organization: &QString,
        org_unit: &QString,
        common_name: &QString,
        email: &QString,
        validity_days: i32,
        key_size: i32,
    ) -> bool {
        let passphrase_str = passphrase.to_string();
        let country_str = country.to_string();
        let state_str = state.to_string();
        let locality_str = locality.to_string();
        let organization_str = organization.to_string();
        let org_unit_str = org_unit.to_string();
        let common_name_str = common_name.to_string();
        let email_str = email.to_string();

        if passphrase_str.is_empty() || common_name_str.is_empty() {
            self.as_mut().set_status_message(QString::from("Passphrase and Common Name are required"));
            return false;
        }

        let dir = certs_dir();
        if let Err(e) = fs::create_dir_all(&dir) {
            let msg = format!("Failed to create ~/certs: {e}");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }

        let pp_path = passphrase_path();
        if let Err(e) = fs::write(&pp_path, &passphrase_str) {
            let msg = format!("Failed to write passphrase file: {e}");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = fs::set_permissions(&pp_path, fs::Permissions::from_mode(0o600));
        // Mirror encrypted copy to devbox.arc (machine-id as key).
        if let Err(e) = encrypt_and_store_passphrase(&passphrase_str) {
            log_debug(&format!("[ducknet] warning: failed to store encrypted passphrase: {}", e));
        }

        self.as_mut().set_status_message(QString::from("Generating CA private key..."));
        self.as_mut().progress_updated(QString::from("Generating CA private key..."));

        let key_path = ca_key_path();
        let key_size_str = key_size.to_string();
        let out = Command::new("openssl")
            .args([
                "genrsa",
                "-aes256",
                "-out",
                &key_path.to_string_lossy(),
                "-passout",
                &format!("file:{}", pp_path.to_string_lossy()),
                &key_size_str,
            ])
            .output();

        match out {
            Ok(o) if o.status.success() => {},
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = format!("openssl genrsa failed: {err}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run openssl genrsa: {e}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));

        self.as_mut().set_status_message(QString::from("Generating CA certificate..."));
        self.as_mut().progress_updated(QString::from("Generating CA certificate..."));

        let pem_path = ca_pem_path();
        // Skip empty fields; openssl rejects empty values.
        let mut subj = String::new();
        if !country_str.is_empty() {
            subj.push_str(&format!("/C={}", country_str));
        }
        if !state_str.is_empty() {
            subj.push_str(&format!("/ST={}", state_str));
        }
        if !locality_str.is_empty() {
            subj.push_str(&format!("/L={}", locality_str));
        }
        if !organization_str.is_empty() {
            subj.push_str(&format!("/O={}", organization_str));
        }
        if !org_unit_str.is_empty() {
            subj.push_str(&format!("/OU={}", org_unit_str));
        }
        subj.push_str(&format!("/CN={}", common_name_str));
        if !email_str.is_empty() {
            subj.push_str(&format!("/emailAddress={}", email_str));
        }

        let validity_str = validity_days.to_string();
        let out = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-new",
                "-nodes",
                "-key",
                &key_path.to_string_lossy(),
                "-sha256",
                "-days",
                &validity_str,
                "-out",
                &pem_path.to_string_lossy(),
                "-passin",
                &format!("file:{}", pp_path.to_string_lossy()),
                "-subj",
                &subj,
            ])
            .output();

        match out {
            Ok(o) if o.status.success() => {},
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = format!("openssl req -x509 failed: {err}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run openssl req: {e}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        let status = QString::from("Setup");
        self.as_mut().set_ca_setup_status(status.clone());
        self.as_mut().set_common_name(common_name.clone());
        self.as_mut().set_status_message(QString::from("CA setup complete. Please install the root certificate."));
        self.as_mut().status_changed(QString::from("CA setup complete"));
        self.as_mut().set_generated_certs(to_qstringlist(&list_generated_certs()));
        true
    }

    fn install_root_cert(mut self: Pin<&mut Self>) -> bool {
        let family = family();
        if family == Family::Unsupported {
            let msg = format!("Unsupported distro '{}' — only Debian and Fedora are supported.", distro_id());
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] install blocked: {}", msg));
            return false;
        }
        let pem_path = ca_pem_path();
        if !pem_path.exists() {
            self.as_mut().set_status_message(QString::from("CA certificate not found. Setup CA first."));
            return false;
        }

        self.as_mut().set_status_message(QString::from("Installing root certificate..."));
        self.as_mut().progress_updated(QString::from("Installing root certificate..."));

        let dest = installed_crt_path_for(&family);
        let update_cmd = update_command_for(&family);
        log_debug(&format!("[ducknet] install_root_cert: distro={:?} pem_path={} dest={} update_cmd={} exists_pem={}", family, pem_path.display(), dest.display(), update_cmd, pem_path.exists()));
        // Absolute path: pkexec polkit skips PATH lookup.
        let out = privileged_output(&[
            "/usr/bin/cp",
            &pem_path.to_string_lossy(),
            &dest.to_string_lossy(),
        ]);
        // Fallback to bare cp when polkit rejects absolute path.
        let out = match out {
            Ok(o) if o.status.success() => Ok(o),
            Ok(o) => {
                let detail = format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
                if detail.contains("No such file") && detail.contains("/usr/bin/cp") {
                    eprintln!("[ducknet] /usr/bin/cp not allowed, retrying bare cp");
                    privileged_output(&[
                        "cp",
                        &pem_path.to_string_lossy(),
                        &dest.to_string_lossy(),
                    ])
                } else {
                    Ok(o)
                }
            }
            Err(e) => Err(e),
        };

        match out {
            Ok(o) if o.status.success() => {
                log_debug("[ducknet] cp succeeded, verifying dest exists...");
            },
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let out_s = String::from_utf8_lossy(&o.stdout);
                let detail = format!("{}{}", err, out_s).trim().to_string();
                log_debug(&format!("[ducknet] cp failed: status={} detail='{}'", o.status, detail));
                let manual_cmd = format!("sudo cp ~/certs/myCA.pem {} && sudo {}", dest.display(), update_cmd);
                let hint: String = if detail.contains("must be setuid") || detail.contains("no new privileges") {
                    format!(" — pkexec blocked (container/no_new_privs). On host run: {}", manual_cmd)
                } else if detail.contains("password is required") || detail.contains("a password is required") {
                    format!(" — passwordless sudo not configured. Either approve the polkit dialog or run: {}", manual_cmd)
                } else if detail.is_empty() {
                    format!(" — auth cancelled or failed (exit {}). Check polkit: pkaction --action-id org.freedesktop.policykit.exec --verbose and journalctl -e | grep polkit", o.status)
                } else {
                    String::new()
                };
                let msg = format!("install failed (cp exit {}): {}{}", o.status, detail, hint);
                log_debug(&format!("[ducknet] {}", msg));
                self.as_mut().set_status_message(QString::from(msg.trim()));
                if !dest.exists() {
                    log_debug(&format!("[ducknet] dest still missing after cp failure: {}", dest.display()));
                }
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run installer (cp): {e} — is pkexec or sudo installed?");
                log_debug(&format!("[ducknet] {}", msg));
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        // Verify copy landed before reporting success.
        if !dest.exists() {
            let msg = format!("Copy reported success but {} not found — check permissions. Returning to setup.", dest.display());
            log_debug(&format!("[ducknet] {}", msg));
            self.as_mut().set_status_message(QString::from(&msg));
            // Stay on Setup so user retries; next launch re-detects status.
            // If you prefer immediate return to page 1 uncomment next line:
            // self.as_mut().set_ca_setup_status(QString::from("NotSetup"));
            return false;
        }
        log_debug(&format!("[ducknet] verified dest exists: {} ({} bytes)", dest.display(), fs::metadata(&dest).map(|m| m.len()).unwrap_or(0)));

        let update_args: Vec<String> = match family {
            Family::Fedora => vec!["/usr/bin/update-ca-trust".to_string(), "update-ca-trust".to_string()],
            Family::Debian => vec!["/usr/sbin/update-ca-certificates".to_string(), "update-ca-certificates".to_string()],
            Family::Unsupported => vec![],
        };
        let mut last_out: Option<std::process::Output> = None;
        let mut succeeded = false;
        let mut last_err: Option<String> = None;
        for cmd in &update_args {
            let out = privileged_output(&[cmd.as_str()]);
            match out {
                Ok(o) if o.status.success() => { succeeded = true; break; },
                Ok(o) => {
                    let detail = format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
                    if detail.contains("No such file") {
                        eprintln!("[ducknet] {} not found, trying next", cmd);
                        last_out = Some(o);
                        continue;
                    } else {
                        last_out = Some(o);
                        break;
                    }
                },
                Err(e) => { last_err = Some(e.to_string()); }
            }
        }
        if !succeeded {
            if let Some(o) = last_out {
                let err = String::from_utf8_lossy(&o.stderr);
                let out_s = String::from_utf8_lossy(&o.stdout);
                let code = o.status.code().unwrap_or(-1);
                let msg = format!("{} failed (exit {code}): {}{}", update_cmd, err, out_s);
                self.as_mut().set_status_message(QString::from(msg.trim()));
                return false;
            } else if let Some(e) = last_err {
                let msg = format!("Failed to run installer ({}): {}", update_cmd, e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        self.as_mut().set_ca_setup_status(QString::from("Installed"));
        self.as_mut().set_status_message(QString::from("Root certificate installed successfully."));
        self.as_mut().status_changed(QString::from("Root certificate installed"));
        true
    }

    fn remove_root_cert(mut self: Pin<&mut Self>) -> bool {
        let family = family();
        if family == Family::Unsupported {
            let msg = format!("Unsupported distro '{}' — only Debian and Fedora are supported.", distro_id());
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] remove blocked: {}", msg));
            return false;
        }
        let dest = installed_crt_path_for(&family);
        let update_cmd = update_command_for(&family);
        if dest.as_os_str().is_empty() {
            self.as_mut().set_status_message(QString::from("No installed certificate path for this distro."));
            return false;
        }
        log_debug(&format!("[ducknet] remove_root_cert: distro={:?} dest={} exists={}", family, dest.display(), dest.exists()));
        self.as_mut().set_status_message(QString::from("Removing root certificate..."));
        self.as_mut().progress_updated(QString::from("Removing root certificate..."));

        let out = privileged_output(&["/usr/bin/rm", "-f", &dest.to_string_lossy()]);
        let out = match out {
            Ok(o) if o.status.success() => Ok(o),
            Ok(o) => {
                let detail = format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
                if detail.contains("No such file") && detail.contains("/usr/bin/rm") {
                    eprintln!("[ducknet] /usr/bin/rm not allowed, retrying bare rm");
                    privileged_output(&["rm", "-f", &dest.to_string_lossy()])
                } else {
                    Ok(o)
                }
            }
            Err(e) => Err(e),
        };
        match out {
            Ok(o) if o.status.success() => {
                log_debug("[ducknet] rm succeeded, verifying removal...");
            },
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let out_s = String::from_utf8_lossy(&o.stdout);
                let detail = format!("{}{}", err, out_s).trim().to_string();
                log_debug(&format!("[ducknet] rm failed: status={} detail='{}'", o.status, detail));
                // Missing file counts as success; still update trust.
                if dest.exists() {
                    let msg = format!("remove failed (rm exit {}): {}", o.status, detail);
                    log_debug(&format!("[ducknet] {}", msg));
                    self.as_mut().set_status_message(QString::from(msg.trim()));
                    return false;
                } else {
                    log_debug("[ducknet] rm reported failure but dest already missing — continuing to update trust");
                }
            },
            Err(e) => {
                let msg = format!("Failed to run installer (rm): {e} — is pkexec or sudo installed?");
                log_debug(&format!("[ducknet] {}", msg));
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        if dest.exists() {
            let msg = format!("Remove reported success but {} still exists — check permissions.", dest.display());
            log_debug(&format!("[ducknet] {}", msg));
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let update_args: Vec<String> = match family {
            Family::Fedora => vec!["/usr/bin/update-ca-trust".to_string(), "update-ca-trust".to_string()],
            Family::Debian => vec!["/usr/sbin/update-ca-certificates".to_string(), "update-ca-certificates".to_string()],
            Family::Unsupported => vec![],
        };
        let mut last_out: Option<std::process::Output> = None;
        let mut succeeded = false;
        let mut last_err: Option<String> = None;
        for cmd in &update_args {
            let out = privileged_output(&[cmd.as_str()]);
            match out {
                Ok(o) if o.status.success() => { succeeded = true; break; },
                Ok(o) => {
                    let detail = format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
                    if detail.contains("No such file") {
                        eprintln!("[ducknet] {} not found, trying next", cmd);
                        last_out = Some(o);
                        continue;
                    } else {
                        last_out = Some(o);
                        break;
                    }
                },
                Err(e) => { last_err = Some(e.to_string()); }
            }
        }
        if !succeeded {
            if let Some(o) = last_out {
                let err = String::from_utf8_lossy(&o.stderr);
                let out_s = String::from_utf8_lossy(&o.stdout);
                let code = o.status.code().unwrap_or(-1);
                let msg = format!("{} failed (exit {code}): {}{}", update_cmd, err, out_s);
                self.as_mut().set_status_message(QString::from(msg.trim()));
                log_debug(&format!("[ducknet] update after remove failed: {}", msg));
                self.as_mut().set_ca_setup_status(QString::from("NotSetup"));
                self.as_mut().status_changed(QString::from("Removed but update failed"));
                return false;
            } else if let Some(e) = last_err {
                let msg = format!("Failed to run installer ({}): {}", update_cmd, e);
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().set_ca_setup_status(QString::from("NotSetup"));
                return false;
            }
        }
        // Old CA invalidates everything tied to it; clean up:
        // 1. Domain certs in ~/certs (+ /etc/hosts entries)
        let domains = list_generated_certs();
        let mut cert_files = 0;
        for d in &domains {
            let (n, errs) = delete_domain_files(d);
            cert_files += n;
            for e in errs {
                log_debug(&format!("[ducknet] regenerate cleanup: {}", e));
            }
            remove_hosts_entry(d);
        }
        // 2. CA key, cert, and secrets (Setup recreates them).
        for p in [ca_key_path(), ca_pem_path(), ca_srl_path(), passphrase_path(), certs_dir().join("bundle.p12")] {
            if p.exists() {
                match fs::remove_file(&p) {
                    Ok(_) => log_debug(&format!("[ducknet] regenerate cleanup removed {}", p.display())),
                    Err(e) => log_debug(&format!("[ducknet] regenerate cleanup failed for {}: {}", p.display(), e)),
                }
            }
        }
        let arc = devbox_state_path();
        if arc.exists() {
            match fs::remove_file(&arc) {
                Ok(_) => log_debug(&format!("[ducknet] regenerate cleanup removed {}", arc.display())),
                Err(e) => log_debug(&format!("[ducknet] regenerate cleanup failed for {}: {}", arc.display(), e)),
            }
        }
        // 3. CA from Firefox NSSDB (non-fatal).
        let firefox_msg = match remove_firefox_ca() {
            Ok(m) => m,
            Err(e) => {
                log_debug(&format!("[ducknet] regenerate cleanup firefox: {}", e));
                format!("Firefox: {}", e)
            }
        };
        self.as_mut().set_generated_certs(QStringList::default());
        self.as_mut().set_common_name(QString::default());
        self.as_mut().set_firefox_ca_installed(is_firefox_ca_installed());
        self.as_mut().set_ca_setup_status(QString::from("NotSetup"));
        let msg = format!(
            "CA removed with {} domain certificate(s) ({} files) + Firefox ({}). Recreate from 1 — Setup.",
            domains.len(),
            cert_files,
            firefox_msg
        );
        self.as_mut().set_status_message(QString::from(&msg));
        self.as_mut().status_changed(QString::from("CA and all certificates removed"));
        log_debug(&format!("[ducknet] remove verified, status set to NotSetup, dest removed: {}", dest.display()));
        true
    }

    fn generate_cert(
        mut self: Pin<&mut Self>,
        domain: &QString,
        sans_list: &QStringList,
        wildcard: bool,
        validity_days: i32,
    ) -> bool {
        let domain_str = domain.to_string();
        if domain_str.is_empty() {
            self.as_mut().set_status_message(QString::from("Domain name is required"));
            return false;
        }
        // Whitelist blocks traversal/metachars from paths, -subj, and .ext.
        if domain_str.parse::<std::net::IpAddr>().is_err() && !valid_cert_domain(&domain_str) {
            self.as_mut().set_status_message(QString::from("Invalid domain name"));
            return false;
        }

        let pem_path = ca_pem_path();
        let key_path = ca_key_path();
        let pp_path = passphrase_path();
        if !pem_path.exists() || !key_path.exists() || !pp_path.exists() {
            self.as_mut().set_status_message(QString::from("CA not setup. Please setup CA first."));
            return false;
        }

        let dir = certs_dir();
        let domain_key = dir.join(format!("{}.key", domain_str));
        let domain_csr = dir.join(format!("{}.csr", domain_str));
        let domain_crt = dir.join(format!("{}.crt", domain_str));
        let domain_ext = dir.join(format!("{}.ext", domain_str));

        self.as_mut().set_status_message(QString::from(&format!("Generating key for {}...", domain_str)));
        self.as_mut().progress_updated(QString::from(&format!("Generating key for {}...", domain_str)));

        let out = Command::new("openssl")
            .args([
                "genrsa",
                "-out",
                &domain_key.to_string_lossy(),
                "2048",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {},
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = format!("openssl genrsa for domain failed: {err}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run openssl genrsa: {e}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        let out = Command::new("openssl")
            .args([
                "req",
                "-new",
                "-key",
                &domain_key.to_string_lossy(),
                "-out",
                &domain_csr.to_string_lossy(),
                "-subj",
                &format!("/CN={}", domain_str),
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {},
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = format!("openssl req for CSR failed: {err}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run openssl req: {e}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        // Collect SANs from QML.
        let mut sans: Vec<String> = Vec::new();
        let raw_sans = sans_list_to_vec(sans_list);
        for s in raw_sans {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                // SANs use same whitelist; IPs take IP.n branch below.
                if trimmed.parse::<std::net::IpAddr>().is_err() && !valid_cert_domain(&trimmed) {
                    self.as_mut().set_status_message(QString::from(&format!("Invalid SAN entry: {}", trimmed)));
                    return false;
                }
                sans.push(trimmed);
            }
        }
        if !sans.contains(&domain_str) {
            sans.insert(0, domain_str.clone());
        }
        if wildcard {
            let wc = if domain_str.starts_with("*.") {
                domain_str.clone()
            } else {
                format!("*.{}", domain_str)
            };
            if !sans.contains(&wc) {
                sans.push(wc);
            }
        }

        let mut ext_content = String::new();
        ext_content.push_str("authorityKeyIdentifier=keyid,issuer\n");
        ext_content.push_str("basicConstraints=CA:FALSE\n");
        ext_content.push_str("keyUsage = digitalSignature, nonRepudiation, keyEncipherment, dataEncipherment\n");
        ext_content.push_str("subjectAltName = @alt_names\n");
        ext_content.push_str("\n[alt_names]\n");
        for (i, san) in sans.iter().enumerate() {
            if san.parse::<std::net::IpAddr>().is_ok() {
                ext_content.push_str(&format!("IP.{} = {}\n", i + 1, san));
            } else {
                ext_content.push_str(&format!("DNS.{} = {}\n", i + 1, san));
            }
        }
        if let Err(e) = fs::write(&domain_ext, &ext_content) {
            let msg = format!("Failed to write ext file: {e}");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }

        self.as_mut().set_status_message(QString::from(&format!("Signing certificate for {}...", domain_str)));
        self.as_mut().progress_updated(QString::from(&format!("Signing certificate for {}...", domain_str)));

        let validity_str = validity_days.to_string();
        let out = Command::new("openssl")
            .args([
                "x509",
                "-req",
                "-in",
                &domain_csr.to_string_lossy(),
                "-CA",
                &pem_path.to_string_lossy(),
                "-CAkey",
                &key_path.to_string_lossy(),
                "-CAcreateserial",
                "-out",
                &domain_crt.to_string_lossy(),
                "-days",
                &validity_str,
                "-sha256",
                "-extfile",
                &domain_ext.to_string_lossy(),
                "-passin",
                &format!("file:{}", pp_path.to_string_lossy()),
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {},
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = format!("openssl x509 signing failed: {err}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run openssl x509: {e}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }

        // Append 127.0.0.1 entry for domain.
        {
            let hosts_path = PathBuf::from("/etc/hosts");
            let hosts_content = fs::read_to_string(&hosts_path).unwrap_or_default();
            // Skip if domain already present as standalone token.
            let already = hosts_content.lines().any(|l| {
                let t = l.trim();
                if t.starts_with('#') || t.is_empty() { return false; }
                t.split_whitespace().any(|w| w == domain_str)
            });
            if !already {
                let entry = format!("127.0.0.1 {}", domain_str);
                // Pass domain as $1 to avoid shell injection.
                let out = privileged_output(&["sh", "-c", "echo \"127.0.0.1 $1\" >> /etc/hosts", "sh", &domain_str]);
                match out {
                    Ok(o) if o.status.success() => {
                        log_debug(&format!("[ducknet] appended to /etc/hosts: {}", entry));
                    }
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr);
                        let out_s = String::from_utf8_lossy(&o.stdout);
                        log_debug(&format!("[ducknet] failed to append to /etc/hosts (exit {}): {}{}", o.status, err, out_s));
                    }
                    Err(e) => {
                        log_debug(&format!("[ducknet] hosts append failed to launch: {}", e));
                    }
                }
            } else {
                log_debug(&format!("[ducknet] /etc/hosts already contains {} — skipping append", domain_str));
            }
        }

        self.as_mut().set_generated_certs(to_qstringlist(&list_generated_certs()));
        self.as_mut().set_status_message(QString::from(&format!("Certificate for {} generated successfully. Reload your web server to serve it (sudo systemctl reload nginx).", domain_str)));
        self.as_mut().status_changed(QString::from(&format!("Generated {}", domain_str)));
        true
    }

    fn delete_cert(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let domain_str = domain.to_string();
        if domain_str.is_empty() {
            self.as_mut().set_status_message(QString::from("Domain name is required"));
            return false;
        }
        if !valid_cert_domain(&domain_str) {
            self.as_mut().set_status_message(QString::from("Invalid domain name"));
            return false;
        }
        let dir = certs_dir();
        // Restrict delete to generated list to block traversal.
        let allowed = list_generated_certs();
        if !allowed.contains(&domain_str) {
            // Fall back to file existence for stale UI.
            let crt = dir.join(format!("{}.crt", domain_str));
            if !crt.exists() {
                self.as_mut().set_status_message(QString::from(&format!("Certificate '{}' not found", domain_str)));
                return false;
            }
        }
        let (removed, errors) = delete_domain_files(&domain_str);
        // myCA.srl is global; keep it.
        if !errors.is_empty() {
            let msg = errors.join("; ");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if removed == 0 {
            self.as_mut().set_status_message(QString::from(&format!("No files found for '{}'", domain_str)));
            return false;
        }
        remove_hosts_entry(&domain_str);
        self.as_mut().set_generated_certs(to_qstringlist(&list_generated_certs()));
        let msg = format!("Deleted certificate '{}' ({} files)", domain_str, removed);
        self.as_mut().set_status_message(QString::from(&msg));
        self.as_mut().status_changed(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn install_to_chrome(mut self: Pin<&mut Self>) -> bool {
        let pem = ca_pem_path();
        if !pem.exists() {
            self.as_mut().set_status_message(QString::from("CA not found. Setup CA first."));
            return false;
        }
        let pass = match decrypt_stored_passphrase() {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("Failed to decrypt passphrase from {}: {} — re-run Setup CA", devbox_state_path().display(), e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let nssdb = PathBuf::from(&home).join(".pki/nssdb");
        if let Err(e) = ensure_nss_db(&nssdb) {
            let msg = format!("Chrome NSS DB init failed {}: {}", nssdb.display(), e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let db_arg = format!("sql:{}", nssdb.display());
        let ca_str = pem.to_string_lossy().to_string();
        // -t "C,," trusts CA for server (Authorities tab).
        let out = Command::new("certutil")
            .args(["-A", "-d", &db_arg, "-n", "My Local Development CA", "-t", "C,,", "-i", &ca_str])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                log_debug(&format!("[ducknet] chrome certutil succeeded: {}", String::from_utf8_lossy(&o.stdout)));
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                if err.contains("already exists") || err.contains("SEC_ERROR") {
                    let _ = Command::new("certutil").args(["-D", "-d", &db_arg, "-n", "My Local Development CA"]).output();
                    let out2 = Command::new("certutil").args(["-A", "-d", &db_arg, "-n", "My Local Development CA", "-t", "C,,", "-i", &ca_str]).output();
                    if let Ok(o2) = out2 {
                        if !o2.status.success() {
                            let msg = format!("Chrome certutil failed after re-add: {}", String::from_utf8_lossy(&o2.stderr).trim());
                            self.as_mut().set_status_message(QString::from(&msg));
                            return false;
                        }
                    }
                } else {
                    let msg = format!("Chrome certutil failed: {}", err.trim());
                    self.as_mut().set_status_message(QString::from(&msg));
                    return false;
                }
            }
            Err(e) => {
                let msg = format!("Failed to run certutil: {} — is nss-tools installed? (apt install libnss3-tools / dnf install nss-tools)", e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        // Skip pk12util: bundle lands in personal certs, not Authorities.
        log_debug("[ducknet] chrome CA installed to Authorities (Bypass pk12util for CA — bundle would go to Your certificates)");
        let _ = &pass; // keep pass for future bundle use if needed
        let _ = ensure_bundle_p12 as fn(&str) -> Option<PathBuf>; // suppress unused warning
        self.as_mut().set_status_message(QString::from("CA installed to Chrome (Authorities) — restart Chrome, check chrome://settings/certificates Authorities"));
        self.as_mut().status_changed(QString::from("Chrome CA installed"));
        true
    }

    /// Install the CA cert into one Firefox profile NSSDB (init DB, add with
    /// -t "C,,", delete + re-add on conflict). Returns the profile label.
    fn install_ca_to_profile(profile: &PathBuf, ca_str: &str) -> Result<String, String> {
        let label = profile.file_name().and_then(|s| s.to_str()).unwrap_or("profile").to_string();
        if let Err(e) = ensure_nss_db(profile) {
            return Err(format!("NSS DB init failed {}: {}", profile.display(), e));
        }
        let db_arg = format!("sql:{}", profile.display());
        let out = Command::new("certutil")
            .args(["-A", "-d", &db_arg, "-n", "My Local Development CA", "-t", "C,,", "-i", ca_str])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                log_debug(&format!("[ducknet] firefox certutil succeeded for {}", profile.display()));
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                if err.contains("already exists") || err.contains("SEC_ERROR") {
                    let _ = Command::new("certutil").args(["-D", "-d", &db_arg, "-n", "My Local Development CA"]).output();
                    let out2 = Command::new("certutil").args(["-A", "-d", &db_arg, "-n", "My Local Development CA", "-t", "C,,", "-i", ca_str]).output();
                    if let Ok(o2) = out2 {
                        if !o2.status.success() {
                            return Err(format!("certutil failed after re-add: {}", String::from_utf8_lossy(&o2.stderr).trim()));
                        }
                    }
                } else {
                    return Err(format!("certutil failed ({}): {}", profile.display(), err.trim()));
                }
            }
            Err(e) => {
                return Err(format!("Failed to run certutil: {} — is nss-tools installed?", e));
            }
        }
        Ok(label)
    }

    fn install_to_firefox(mut self: Pin<&mut Self>) -> bool {
        let pem = ca_pem_path();
        if !pem.exists() {
            self.as_mut().set_status_message(QString::from("CA not found. Setup CA first."));
            return false;
        }
        let pass = match decrypt_stored_passphrase() {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("Failed to decrypt passphrase from {}: {} — re-run Setup CA", devbox_state_path().display(), e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        let profiles = find_firefox_profiles();
        if profiles.is_empty() {
            let msg = "No Firefox profile found at ~/.mozilla/firefox or ~/.config/mozilla/firefox (*.default-release / *.default) — launch Firefox once to create a profile".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let ca_str = pem.to_string_lossy().to_string();
        let mut installed: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for profile in &profiles {
            match Self::install_ca_to_profile(profile, &ca_str) {
                Ok(label) => installed.push(label),
                Err(e) => failed.push(format!("{}: {}", profile.file_name().and_then(|s| s.to_str()).unwrap_or("profile"), e)),
            }
        }
        log_debug("[ducknet] firefox CA installed to Authorities (skip pk12util for CA)");
        let _ = &pass;
        let _ = ensure_bundle_p12 as fn(&str) -> Option<PathBuf>;
        if installed.is_empty() {
            let msg = format!("Firefox install failed — {}", failed.join("; "));
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let mut msg = format!("CA installed to Firefox ({}) — check Firefox Settings → Privacy & Security → Certificates → Authorities", installed.join(", "));
        if !failed.is_empty() {
            msg.push_str(&format!(" [skipped: {}]", failed.join("; ")));
        }
        self.as_mut().set_status_message(QString::from(&msg));
        self.as_mut().set_firefox_ca_installed(is_firefox_ca_installed());
        self.as_mut().status_changed(QString::from(&msg));
        true
    }

    fn refresh_firefox_status(mut self: Pin<&mut Self>) -> bool {
        let installed = is_firefox_ca_installed();
        self.as_mut().set_firefox_ca_installed(installed);
        log_debug(&format!("[ducknet] refreshFirefoxStatus -> {}", installed));
        installed
    }

    fn delete_from_chrome(mut self: Pin<&mut Self>) -> bool {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let nssdb = PathBuf::from(&home).join(".pki/nssdb");
        let db_arg = format!("sql:{}", nssdb.display());
        // Step 1: list nicknames for debug.
        let _find = Command::new("certutil").args(["-L", "-d", &db_arg]).output();
        if let Ok(o) = _find {
            log_debug(&format!("[ducknet] chrome certutil -L before delete: {}", String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.contains("My Local")).collect::<Vec<_>>().join(", ")));
        }
        // Step 2: Delete
        let out = Command::new("certutil")
            .args(["-D", "-d", &db_arg, "-n", "My Local Development CA"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                log_debug("[ducknet] chrome certutil -D succeeded");
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                log_debug(&format!("[ducknet] chrome certutil -D failed: {}", err));
                if err.contains("not found") || err.contains("SEC_ERROR_UNKNOWN_CERT") || err.contains("PR_NOT_FOUND") || err.contains("could not find") {
                    let msg = "CA not found in Chrome NSSDB — already removed. Check chrome://settings/certificates Manage local certificates after restart.".to_string();
                    self.as_mut().set_status_message(QString::from(&msg));
                    log_debug("[ducknet] chrome CA already absent");
                    return true;
                }
                let msg = format!("Chrome delete failed: {}", err);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            Err(e) => {
                let msg = format!("Failed to run certutil: {} — is nss-tools installed? (apt install libnss3-tools / dnf install nss-tools)", e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        // Step 3: Verify
        let verify = Command::new("certutil").args(["-L", "-d", &db_arg]).output();
        if let Ok(o) = verify {
            let out_str = String::from_utf8_lossy(&o.stdout);
            if !out_str.contains("My Local Development CA") {
                let msg = "Removed CA from Chrome (NSSDB) — verify with certutil -L -d sql:$HOME/.pki/nssdb and restart Chrome (chrome://settings/certificates)".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().status_changed(QString::from(&msg));
                log_debug("[ducknet] chrome delete verified via certutil -L");
                return true;
            }
        }
        let msg = "Delete command succeeded but CA still listed in certutil -L -d sql:$HOME/.pki/nssdb — try certutil -L to find exact nickname (case-sensitive)".to_string();
        self.as_mut().set_status_message(QString::from(&msg));
        false
    }

    fn delete_from_firefox(mut self: Pin<&mut Self>) -> bool {
        match remove_firefox_ca() {
            Ok(msg) => {
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().set_firefox_ca_installed(is_firefox_ca_installed());
                self.as_mut().status_changed(QString::from(&msg));
                true
            }
            Err(e) => {
                self.as_mut().set_status_message(QString::from(&e));
                self.as_mut().set_firefox_ca_installed(is_firefox_ca_installed());
                false
            }
        }
    }
}

fn sans_list_to_vec(list: &QStringList) -> Vec<String> {
    // QStringList API varies; parse Debug output for quoted entries.
    // Caller still adds primary domain, so empty fallback is safe.
    let debug = format!("{:?}", list);

    let mut result = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    let mut escape = false;
    for ch in debug.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            if in_quote {
                result.push(current.clone());
                current.clear();
                in_quote = false;
            } else {
                in_quote = true;
            }
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    result.retain(|s| !s.is_empty() && s != "QStringList" && s != "QList");
    result
}

impl Default for DevCertsManagerRust {
    fn default() -> Self {
        let family = family();
        let (status_msg, ca_status) = if family == Family::Unsupported {
            (format!("Unsupported distro '{}' — only Debian and Fedora are supported.", distro_id()), "Unsupported".to_string())
        } else {
            ("Ready".to_string(), detect_ca_status())
        };
        Self {
            ca_setup_status: QString::from(&ca_status),
            common_name: QString::from(&detect_common_name()),
            generated_certs: to_qstringlist(&list_generated_certs()),
            status_message: QString::from(&status_msg),
            distro_family: QString::from(family_str(&family)),
            distro_id: QString::from(&distro_id()),
            installed_cert_path: QString::from(installed_crt_path_for(&family).to_string_lossy().to_string()),
            update_command: QString::from(update_command_for(&family)),
            firefox_ca_installed: if family == Family::Unsupported {
                false
            } else {
                is_firefox_ca_installed()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serializes HOME-swapping tests; env vars are process-global.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Fake HOME where Default=1 points at .default but browser runs .default-release.
    /// Unique dir per call; tests share HOME across threads.
    fn setup_fake_firefox_home() -> (PathBuf, Option<String>) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("ducknet-test-ff-{}-{}", std::process::id(), n));
        let base = root.join(".mozilla/firefox");
        let def = base.join("abcd1234.default");
        let rel = base.join("wxyz5678.default-release");
        let esr = base.join("efgh5678.default-esr");
        fs::create_dir_all(&def).unwrap();
        fs::create_dir_all(&rel).unwrap();
        fs::create_dir_all(&esr).unwrap();
        fs::write(
            base.join("profiles.ini"),
            "[Profile0]\nName=default\nIsRelative=1\nPath=abcd1234.default\nDefault=1\n\n\
             [Profile1]\nName=default-release\nIsRelative=1\nPath=wxyz5678.default-release\n\n\
             [Install0123456789ABCDEF]\nDefault=efgh5678.default-esr\nLocked=1\n",
        )
        .unwrap();
        let old = std::env::var("HOME").ok();
        std::env::set_var("HOME", &root);
        (root, old)
    }

    fn restore_home(old: Option<String>) {
        match old {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn cert_domain_validation() {
        assert!(valid_cert_domain("myapp.test"));
        assert!(valid_cert_domain("my-app_1.test"));
        assert!(valid_cert_domain("*.myapp.test"));
        assert!(!valid_cert_domain(""));
        assert!(!valid_cert_domain("../evil"));
        assert!(!valid_cert_domain("a/b"));
        assert!(!valid_cert_domain("a\\b"));
        assert!(!valid_cert_domain("a;b"));
        assert!(!valid_cert_domain("a b"));
        assert!(!valid_cert_domain("a$HOME"));
        assert!(!valid_cert_domain("*."));
    }

    #[test]
    fn firefox_discovers_both_default_profiles() {
        let _guard = HOME_LOCK.lock().unwrap();
        let (root, old) = setup_fake_firefox_home();
        let profiles = find_firefox_profiles();
        restore_home(old);
        let names: Vec<String> = profiles
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .collect();
        assert!(names.contains(&"abcd1234.default".to_string()), "missing .default: {:?}", names);
        assert!(names.contains(&"wxyz5678.default-release".to_string()), "missing .default-release: {:?}", names);
        assert!(names.contains(&"efgh5678.default-esr".to_string()), "missing .default-esr (Install section): {:?}", names);
        assert_eq!(names.len(), 3, "no duplicates expected: {:?}", names);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn firefox_status_false_without_nssdb() {
        // No cert9.db here, so certutil reports not installed.
        let _guard = HOME_LOCK.lock().unwrap();
        let (root, old) = setup_fake_firefox_home();
        let installed = is_firefox_ca_installed();
        restore_home(old);
        assert!(!installed);
        let _ = fs::remove_dir_all(&root);
    }
}
