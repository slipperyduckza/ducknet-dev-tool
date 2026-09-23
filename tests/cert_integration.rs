use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn tmp_certs() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ducknet-test-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    dir
}

#[test]
fn openssl_workflow_aes256_and_san() {
    let certs = tmp_certs();
    let pass = certs.join(".ca_passphrase");
    fs::write(&pass, "testpass").unwrap();
    fs::set_permissions(&pass, fs::Permissions::from_mode(0o600)).unwrap();

    let key = certs.join("myCA.key");
    let pem = certs.join("myCA.pem");

    // CA key.
    let out = Command::new("openssl")
        .args([
            "genrsa", "-aes256",
            "-out", &key.to_string_lossy(),
            "-passout", &format!("file:{}", pass.to_string_lossy()),
            "2048",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "genrsa failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(key.exists());

    // CA cert.
    let out = Command::new("openssl")
        .args([
            "req", "-x509", "-new", "-nodes",
            "-key", &key.to_string_lossy(),
            "-sha256", "-days", "1825",
            "-out", &pem.to_string_lossy(),
            "-passin", &format!("file:{}", pass.to_string_lossy()),
            "-subj", "/C=US/ST=California/L=San Francisco/O=DuckNet/OU=Dev/CN=DuckNet Dev CA/emailAddress=dev@ducknet.test",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "req x509 failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(pem.exists());

    // Domain cert.
    let domain = "myapp.test";
    let dkey = certs.join(format!("{}.key", domain));
    let csr = certs.join(format!("{}.csr", domain));
    let crt = certs.join(format!("{}.crt", domain));
    let ext = certs.join(format!("{}.ext", domain));

    let out = Command::new("openssl").args(["genrsa", "-out", &dkey.to_string_lossy(), "2048"]).output().unwrap();
    assert!(out.status.success());

    let out = Command::new("openssl")
        .args(["req", "-new", "-key", &dkey.to_string_lossy(), "-out", &csr.to_string_lossy(), "-subj", &format!("/CN={}", domain)])
        .output()
        .unwrap();
    assert!(out.status.success());

    let ext_content = "authorityKeyIdentifier=keyid,issuer\nbasicConstraints=CA:FALSE\nkeyUsage = digitalSignature, nonRepudiation, keyEncipherment, dataEncipherment\nsubjectAltName = @alt_names\n\n[alt_names]\nDNS.1 = myapp.test\nDNS.2 = *.myapp.test\nIP.1 = 192.168.1.10\n";
    fs::write(&ext, ext_content).unwrap();

    let out = Command::new("openssl")
        .args([
            "x509", "-req",
            "-in", &csr.to_string_lossy(),
            "-CA", &pem.to_string_lossy(),
            "-CAkey", &key.to_string_lossy(),
            "-CAcreateserial",
            "-out", &crt.to_string_lossy(),
            "-days", "825", "-sha256",
            "-extfile", &ext.to_string_lossy(),
            "-passin", &format!("file:{}", pass.to_string_lossy()),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "x509 failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(crt.exists());

    // Verify SANs.
    let out = Command::new("openssl").args(["x509", "-noout", "-text", "-in", &crt.to_string_lossy()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("DNS:myapp.test"));
    assert!(text.contains("DNS:*.myapp.test"));
    assert!(text.contains("192.168.1.10"));

    let _ = fs::remove_dir_all(&certs);
}
