use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn certs_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join("certs")
}

fn main() {
    println!("=== Live Real Cert Test ===");
    println!("Testing the exact OpenSSL workflow from src/devcerts.rs in ~/certs");
    
    let certs = certs_dir();
    let pass = certs.join(".ca_passphrase");
    let key = certs.join("myCA.key");
    let pem = certs.join("myCA.pem");
    
    // Remove previous test dir for a clean run.
    if certs.exists() {
        println!("~/certs already exists, contents:");
        for entry in fs::read_dir(&certs).unwrap().flatten() {
            println!("  {}", entry.path().display());
        }
        println!("Removing for clean live test...");
        let _ = fs::remove_dir_all(&certs);
    }
    fs::create_dir_all(&certs).unwrap();
    
    let passphrase = "liveTest123";
    fs::write(&pass, passphrase).unwrap();
    fs::set_permissions(&pass, fs::Permissions::from_mode(0o600)).unwrap();
    println!("1. Created passphrase file: {}", pass.display());
    
    // CA key.
    println!("2. Generating CA key (2048, aes256)...");
    let out = Command::new("openssl")
        .args(["genrsa", "-aes256", "-out", &key.to_string_lossy(), "-passout", &format!("file:{}", pass.to_string_lossy()), "2048"])
        .output().unwrap();
    assert!(out.status.success(), "genrsa failed: {}", String::from_utf8_lossy(&out.stderr));
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    println!("   -> {}", key.display());
    
    // CA cert.
    println!("3. Generating CA pem (1825 days)...");
    let out = Command::new("openssl")
        .args(["req", "-x509", "-new", "-nodes",
               "-key", &key.to_string_lossy(),
               "-sha256", "-days", "1825",
               "-out", &pem.to_string_lossy(),
               "-passin", &format!("file:{}", pass.to_string_lossy()),
               "-subj", "/C=US/ST=California/L=San Francisco/O=DuckNet/OU=Dev/CN=DuckNet Live CA/emailAddress=live@ducknet.test"])
        .output().unwrap();
    assert!(out.status.success(), "req failed: {}", String::from_utf8_lossy(&out.stderr));
    println!("   -> {}", pem.display());
    
    // Subdomain cases mirroring the app.
    let domains = vec![
        ("myapp.test", vec!["api.myapp.test", "app.myapp.test"], false),
        ("wildcard.test", vec![], true),
        ("multi.test", vec!["a.multi.test", "b.multi.test", "192.168.1.10"], false),
    ];
    
    for (domain, sans, wildcard) in domains {
        println!("\n4. Generating cert for {} (wild={}, sans={:?})...", domain, wildcard, sans);
        let dkey = certs.join(format!("{}.key", domain));
        let csr = certs.join(format!("{}.csr", domain));
        let crt = certs.join(format!("{}.crt", domain));
        let ext = certs.join(format!("{}.ext", domain));
        
        let out = Command::new("openssl").args(["genrsa", "-out", &dkey.to_string_lossy(), "2048"]).output().unwrap();
        assert!(out.status.success());
        
        let out = Command::new("openssl")
            .args(["req", "-new", "-key", &dkey.to_string_lossy(), "-out", &csr.to_string_lossy(), "-subj", &format!("/CN={}", domain)])
            .output().unwrap();
        assert!(out.status.success());
        
        // Build SAN list as devcerts.rs does.
        let mut all_sans = vec![domain.to_string()];
        for s in &sans { all_sans.push(s.to_string()); }
        if wildcard {
            let wc = if domain.starts_with("*.") { domain.to_string() } else { format!("*.{}", domain) };
            all_sans.push(wc);
        }
        
        let mut ext_content = String::new();
        ext_content.push_str("authorityKeyIdentifier=keyid,issuer\nbasicConstraints=CA:FALSE\nkeyUsage = digitalSignature, nonRepudiation, keyEncipherment, dataEncipherment\nsubjectAltName = @alt_names\n\n[alt_names]\n");
        for (i, san) in all_sans.iter().enumerate() {
            if san.parse::<std::net::IpAddr>().is_ok() {
                ext_content.push_str(&format!("IP.{} = {}\n", i+1, san));
            } else {
                ext_content.push_str(&format!("DNS.{} = {}\n", i+1, san));
            }
        }
        fs::write(&ext, &ext_content).unwrap();
        println!("   ext: {}", ext_content.replace("\n", " | "));
        
        let out = Command::new("openssl")
            .args(["x509", "-req", "-in", &csr.to_string_lossy(),
                   "-CA", &pem.to_string_lossy(), "-CAkey", &key.to_string_lossy(),
                   "-CAcreateserial", "-out", &crt.to_string_lossy(),
                   "-days", "825", "-sha256",
                   "-extfile", &ext.to_string_lossy(),
                   "-passin", &format!("file:{}", pass.to_string_lossy())])
            .output().unwrap();
        assert!(out.status.success(), "x509 failed: {}", String::from_utf8_lossy(&out.stderr));
        println!("   -> {} (SANs: {})", crt.display(), all_sans.join(", "));
        
        // Verify SANs.
        let out = Command::new("openssl").args(["x509", "-noout", "-text", "-in", &crt.to_string_lossy()]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        for san in &all_sans {
            if san.parse::<std::net::IpAddr>().is_ok() {
                assert!(text.contains(san), "IP SAN {} not in cert", san);
            } else {
                assert!(text.contains(san), "DNS SAN {} not in cert", san);
            }
        }
        println!("   verify: OK");
    }
    
    println!("\n=== Live test complete ===");
    println!("Generated files in ~/certs:");
    for entry in fs::read_dir(&certs).unwrap().flatten() {
        let meta = entry.metadata().unwrap();
        println!("  {:20} {:6} bytes", entry.file_name().to_string_lossy(), meta.len());
    }
    println!("\nTo test install (requires passwordless sudo):");
    println!("  sudo cp ~/certs/myCA.pem /usr/local/share/ca-certificates/myCA.crt && sudo update-ca-certificates");
    println!("\nTo view a cert: openssl x509 -noout -text -in ~/certs/myapp.test.crt | grep -A2 \"Subject Alternative Name\"");
}
