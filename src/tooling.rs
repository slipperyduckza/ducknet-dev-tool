//! Setup Tooling backend: passwordless sudo (console) + passwordless pkexec (Plasma).
//! Probes stay unprivileged and never prompt; mutations go through `privileged_output`.

use cxx_qt_lib::QString;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::common::{
    binary_present, dev_username, distro_id, distro_is_debian_like, distro_is_fedora_like,
    home_dir_for, log_debug, op_log_tail, privileged_output, privileged_spawn, service_group,
    sudo_nopasswd_active, valid_username, DEBIAN_DEFAULT_SITE_CONTENT, FEDORA_NGINX_CONF_TEMPLATE,
    NEEDS_ROOT,
};

const DEBIAN_RULES_FILE: &str = "/etc/polkit-1/rules.d/49-sudo-nopassword.rules";
const FEDORA_RULES_FILE: &str = "/etc/polkit-1/rules.d/49-wheel-nopassword.rules";

/// Exact package sets from the Install NGINX PHP spec; uninstall removes this same list.
const DEBIAN_NGINX_PHP_PKGS: &[&str] = &[
    "nginx-full",
    "php8.4", "php8.4-cli", "php8.4-fpm", "php8.4-common", "php8.4-mbstring",
    "php8.4-xml", "php8.4-gd", "php8.4-curl", "php8.4-zip", "php8.4-bcmath",
    "php8.4-mysql", "php8.4-intl", "php8.4-imagick", "php8.4-redis",
    "php8.4-soap", "php8.4-memcached", "php8.4-apcu", "php-pgsql", "php-pear",
];
const FEDORA_NGINX_PHP_PKGS: &[&str] = &[
    "nginx", "nginx-all-modules",
    "php", "php-cli", "php-fpm", "php-common", "php-opcache", "php-mbstring",
    "php-xml", "php-gd", "php-curl", "php-zip", "php-bcmath", "php-mysqlnd",
    // Remi only ships `php-pecl-imagick-im7`; the generic name pulls Fedora's 8.5-linked
    // build, which never resolves against the php:remi-8.4 stream.
    "php-intl", "php-pecl-imagick-im7", "php-pecl-redis6", "php-soap",
    "php-pecl-memcached", "php-pecl-apcu", "php-pecl-json-post",
    "php-pecl-jsonpath", "php-pecl-oauth", "php-pgsql", "php-pear",
];

/// Remi RPM provides PHP 8.4 via the php:remi-8.4 stream; `$(rpm -E %fedora)` resolves release at run time.
const FEDORA_REMI_RPM: &str =
    "https://rpms.remirepo.net/fedora/remi-release-$(rpm -E %fedora).rpm";

/// Dev-environment toolchains. rustup runs as the dev user via `sudo -u`, never root.
const DEBIAN_DEVENV_PKGS: &[&str] = &[
    "build-essential",
    "python3", "python3-dev", "python3-pip", "python3-venv",
    "dkms", "curl", "wget",
    "clang", "clangd", "lld", "lldb", "clang-format", "clang-tidy",
    "cmake", "ninja-build", "gdb", "valgrind", "git", "manpages-dev",
    "gcc", "g++",
];
const FEDORA_DEVENV_GROUPS: &[&str] = &["@c-development", "@development-tools"];
const FEDORA_DEVENV_PKGS: &[&str] = &[
    "python3", "python3-devel", "python3-pip",
    "dkms", "curl", "wget", "git",
    "clang", "lld", "lldb", "clang-tools-extra",
    "cmake", "ninja-build", "gdb", "valgrind", "man-pages",
    "gcc", "gcc-c++",
];
/// Official rustup installer; unquoted URL keeps the nested `sh -c` quoting safe.
const RUSTUP_PIPE: &str =
    "curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y";

/// Prerequisites for fresh installs: `certutil` lives in libnss3-tools (Debian) / nss-tools (Fedora).
const DEBIAN_PREREQ_PKGS: &[&str] = &["pkexec", "libnss3-tools"];
const FEDORA_PREREQ_PKGS: &[&str] = &["nss-tools"];

/// VSCodium repo setup (upstream: paulcarroty/vscodium-deb-rpm-repo). URLs/names stay
/// hardcoded so no untrusted input reaches the shell.
const VSCODIUM_GPG_URL: &str =
    "https://gitlab.com/paulcarroty/vscodium-deb-rpm-repo/raw/master/pub.gpg";
const VSCODIUM_DEB_KEYRING: &str = "/usr/share/keyrings/vscodium-archive-keyring.gpg";
const VSCODIUM_DEB_SOURCES: &str = "/etc/apt/sources.list.d/vscodium.sources";
const VSCODIUM_RPM_REPO: &str = "/etc/yum.repos.d/vscodium.repo";

/// Opencode CLI installer (universal); runs as the dev user so it lands in ~/.opencode.
const OPENCODE_PIPE: &str = "curl -fsSL https://opencode.ai/install | bash";

/// Staged sudo-enrollment script (prompts for ROOT via `su`; reboot stays chained so a failed adduser never reboots).
const FIXSUDO_STAGED_PATH: &str = "/tmp/ducknet-fixsudo.sh";

const PKG_OP_LOG: &str = "/tmp/ducknet-pkg-op.log";

/// Background package op (apt/dnf runs for minutes); QML polls `poll_package_op`.
struct PkgOp {
    child: Child,
    title: String,
    done: bool,
    ok: bool,
    code: Option<i32>,
}

static PKG_OP: Mutex<Option<PkgOp>> = Mutex::new(None);

/// Reads the polkit rule file without prompting; falls back to `sudo -n cat`.
fn read_rules_file(path: &str) -> Option<String> {
    if let Ok(c) = std::fs::read_to_string(path) {
        return Some(c);
    }
    Command::new("sudo")
        .arg("-n")
        .arg("cat")
        .arg(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Pure parser for `dpkg -s` output.
fn dpkg_status_installed(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.trim() == "Status: install ok installed")
}

/// pkexec presence: `dpkg -s pkexec` on Debian-family, PATH lookup elsewhere.
fn pkexec_installed() -> bool {
    if distro_is_debian_like() {
        if let Ok(out) = Command::new("dpkg").arg("-s").arg("pkexec").output() {
            if out.status.success()
                && dpkg_status_installed(&String::from_utf8_lossy(&out.stdout))
            {
                return true;
            }
        }
        return false;
    }
    crate::common::path_lookup("pkexec").is_some()
}
/// Privileged argv for Install/Uninstall NGINX+PHP, DevEnv, VSCodium, Opencode.
/// Packages stay hardcoded and `user` stays charset-validated.
fn package_command(variant: &str, op: &str, user: &str) -> Option<Vec<String>> {
    match (variant, op) {
        ("debian", "install") => {
            let group = service_group(user);
            let mut script = format!(
                "export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y {}",
                DEBIAN_NGINX_PHP_PKGS.join(" ")
            );
            // Services run as the dev user (else ~/WebRoots stays unreadable); failures must fail the op honestly.
            if let Some(cfg) = service_user_script(variant, user, &group) {
                script.push_str(" && ");
                script.push_str(&cfg);
            }
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        ("debian", "uninstall") => {
            let script = format!(
                "export DEBIAN_FRONTEND=noninteractive; apt-get remove -y {}",
                DEBIAN_NGINX_PHP_PKGS.join(" ")
            );
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        ("fedora", "install") => {
            let group = service_group(user);
            // PHP 8.4 comes from the php:remi-8.4 stream; verification stays in the && chain so a broken repo fails honestly.
            let mut steps = vec![
                format!("dnf install -y \"{}\"", FEDORA_REMI_RPM),
                "rpm -q remi-release".to_string(),
                "dnf repo list --enabled | grep -E '^remi'".to_string(),
                "rpm -ql remi-release | grep -E '/etc/yum.repos.d/|/etc/pki/rpm-gpg/'".to_string(),
                "dnf makecache".to_string(),
                "dnf module reset php -y".to_string(),
                "dnf module enable php:remi-8.4 -y".to_string(),
                format!("dnf install -y {}", FEDORA_NGINX_PHP_PKGS.join(" ")),
            ];
            // Conform to the Debian-style layout first (backup captures
            // pristine stock), then the service-user rewrite, then enable + start.
            steps.push(fedora_conform_fragment(user));
            if let Some(cfg) = service_user_script(variant, user, &group) {
                steps.push(cfg);
            }
            steps.push("systemctl enable --now nginx.service".to_string());
            steps.push("systemctl enable --now php-fpm.service".to_string());
            Some(vec!["sh".to_string(), "-c".to_string(), steps.join(" && ")])
        }
        ("fedora", "uninstall") => {
            let mut argv = vec!["dnf".to_string(), "remove".to_string(), "-y".to_string()];
            argv.extend(FEDORA_NGINX_PHP_PKGS.iter().map(|p| p.to_string()));
            Some(argv)
        }
        // DevEnv: rustup runs as the dev user (root's HOME would trap it in /root/.cargo).
        ("debian", "devenv") => {
            let script = format!(
                "export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y {} && sudo -u {} sh -c \"{}\"",
                DEBIAN_DEVENV_PKGS.join(" "),
                user,
                RUSTUP_PIPE
            );
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        ("fedora", "devenv") => {
            let pkgs: Vec<String> = FEDORA_DEVENV_GROUPS
                .iter()
                .chain(FEDORA_DEVENV_PKGS.iter())
                .map(|p| p.to_string())
                .collect();
            let script = format!(
                "dnf install -y {} && sudo -u {} sh -c \"{}\"",
                pkgs.join(" "),
                user,
                RUSTUP_PIPE
            );
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        // VSCodium: repo overwrite with `>` stays idempotent; `-y` everywhere because stdin is null.
        ("debian", "vscodium") => {
            let script = format!(
                "export DEBIAN_FRONTEND=noninteractive; wget -qO - {gpg} | gpg --dearmor | dd of={keyring} && printf '%s\\n' 'Types: deb' 'URIs: https://download.vscodium.com/debs' 'Suites: vscodium' 'Components: main' 'Architectures: amd64 arm64' 'Signed-by: {keyring}' > {sources} && apt-get update && apt-get install -y codium",
                gpg = VSCODIUM_GPG_URL,
                keyring = VSCODIUM_DEB_KEYRING,
                sources = VSCODIUM_DEB_SOURCES,
            );
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        ("fedora", "vscodium") => {
            let script = format!(
                "printf '%s\\n' '[gitlab.com_paulcarroty_vscodium_repo]' 'name=gitlab.com_paulcarroty_vscodium_repo' 'baseurl=https://paulcarroty.gitlab.io/vscodium-deb-rpm-repo/rpms/' 'enabled=1' 'gpgcheck=1' 'repo_gpgcheck=1' 'gpgkey={gpg}' 'metadata_expire=1h' > {repo} && dnf install -y codium",
                gpg = VSCODIUM_GPG_URL,
                repo = VSCODIUM_RPM_REPO,
            );
            Some(vec!["sh".to_string(), "-c".to_string(), script])
        }
        // Opencode: universal pipe as the dev user.
        ("debian", "opencode") => Some(opencode_command(user)),
        ("fedora", "opencode") => Some(opencode_command(user)),
        _ => None,
    }
}

/// argv for the Opencode install; `user` stays charset-validated by the caller.
fn opencode_command(user: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("sudo -u {} sh -c \"{}\"", user, OPENCODE_PIPE),
    ]
}

/// Parses `groups` output; only an exact `sudo` token counts.
fn groups_has_sudo(output: &str) -> bool {
    output
        .split_whitespace()
        .any(|tok| tok.trim_end_matches(':') == "sudo")
}

/// Unprivileged `groups` probe; Debian fresh installs omit the sudo group.
fn in_sudo_group() -> bool {
    Command::new("groups")
        .output()
        .map(|o| o.status.success() && groups_has_sudo(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

/// `certutil` green-LED probe: PATH, known locations, then the whereis database.
fn certutil_present() -> bool {
    binary_present(
        "certutil",
        &["/usr/bin/certutil", "/bin/certutil", "/usr/local/bin/certutil"],
    )
}

/// Prerequisite (label, packages, installer) for this distro.
fn prereq_selection() -> Option<(&'static str, &'static [&'static str], &'static str)> {
    if distro_is_debian_like() {
        Some(("Debian", DEBIAN_PREREQ_PKGS, "apt-get"))
    } else if distro_is_fedora_like() {
        Some(("Fedora", FEDORA_PREREQ_PKGS, "dnf"))
    } else {
        None
    }
}

/// Prerequisite install body for a terminal `sudo` shell; Fedora also disables SELinux.
/// `setenforce` stays best-effort; `reboot` reboots on success instead of holding the window open.
fn prereq_install_script(variant: &str, pkgs: &[&str], reboot: bool) -> Option<String> {
    match variant {
        "debian" => {
            let install = format!(
                "export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y {}",
                pkgs.join(" ")
            );
            let tail = if reboot {
                // Reboot stays inside the sudo shell; nothing after it ever runs.

                " && echo \"Rebooting now to make the change permanent.\" && reboot".to_string()
            } else {
                // Hold runs unprivileged outside sudo so the leftover window is never root's; double quotes only inside `sudo sh -c '...'`.
                "; echo; echo \"Prerequisites finished — you may close this window.\"; exec sh"
                    .to_string()
            };
            Some(format!("sudo sh -c '{install}{tail}'"))
        }
        "fedora" => {
            let base = format!("dnf install -y {}", pkgs.join(" "));
            let live_off = "{ setenforce 0 || echo \"NOTE: setenforce skipped (SELinux already disabled or unavailable).\"; }";
            let selinux_check = "grep -Eqi \"^[[:space:]]*SELINUX=disabled([[:space:]]*(#.*)?)?$\" /etc/selinux/config";
            let disable =
                "sed -i \"s/^SELINUX=.*/SELINUX=disabled/\" /etc/selinux/config";
            if reboot {
                // Runtime guard: never reboot an already-disabled machine; reboot stays inside sudo.
                let script = format!(
                    "{base} && if {selinux_check}; then echo \"SELinux already disabled — no reboot needed.\"; {live_off}; else {disable} && {live_off} && echo \"Rebooting now to make the change permanent.\" && reboot; fi"
                );
                Some(format!("sudo sh -c '{script}'"))
            } else {
                let script = format!("{base} && {disable} && {live_off}");
                Some(format!(
                    "sudo sh -c '{script}; echo; echo \"Prerequisites finished — you may close this window.\"; exec sh'"
                ))
            }
        }
        _ => None,
    }
}

/// Staged sudo-enrollment body; no-ops when already in the sudo group.
fn fixsudo_script_content(user: &str) -> String {
    format!(
        "#!/bin/sh\nCURRENT_USER={user}\n/usr/bin/echo \"Checking if $CURRENT_USER is already in the sudo group...\"\nif /usr/bin/id -nG \"$CURRENT_USER\" | /usr/bin/tr \" \" \"\\n\" | /usr/bin/grep -qx sudo; then\n/usr/bin/echo \"$CURRENT_USER is already in the sudo group -- no changes needed.\"\nexit 0\nfi\n/usr/bin/echo \"Please enter the ROOT password below to grant $CURRENT_USER sudo privileges:\"\n/usr/bin/su - root -c \"/usr/sbin/adduser $CURRENT_USER sudo && reboot\"\n"
    )
}

fn user_places_path(home: &str) -> String {
    format!("{}/.local/share/user-places.xbel", home.trim_end_matches('/'))
}

/// Dolphin/Places bookmark blocks for `user`.
fn dev_places_snippet(user: &str) -> String {
    format!(
        "<bookmark href=\"file:///home/{user}/Coding\">\n <title>Coding</title>\n <info>\n  <metadata owner=\"http://freedesktop.org\">\n   <bookmark:icon name=\"folder-script\"/>\n  </metadata>\n  <metadata owner=\"http://kde.org\">\n   <ID>1711111111</ID>\n   <isSystemItem>false</isSystemItem>\n  </metadata>\n </info>\n</bookmark>\n<bookmark href=\"file:///home/{user}/MyApps\">\n <title>MyApps</title>\n <info>\n  <metadata owner=\"http://freedesktop.org\">\n   <bookmark:icon name=\"folder-extension\"/>\n  </metadata>\n  <metadata owner=\"http://kde.org\">\n   <ID>1711111112</ID>\n   <isSystemItem>false</isSystemItem>\n  </metadata>\n </info>\n</bookmark>\n<bookmark href=\"file:///home/{user}/WebRoots\">\n <title>WebRoots</title>\n <info>\n  <metadata owner=\"http://freedesktop.org\">\n   <bookmark:icon name=\"folder-html\"/>\n  </metadata>\n  <metadata owner=\"http://kde.org\">\n   <ID>1711111113</ID>\n   <isSystemItem>false</isSystemItem>\n  </metadata>\n </info>\n</bookmark>\n"
    )
}

/// True iff all three dev hrefs are present; href-based so a same-titled bookmark elsewhere never counts.
fn dev_places_present(content: &str, user: &str) -> bool {
    content.contains(&format!("file:///home/{}/Coding", user))
        && content.contains(&format!("file:///home/{}/MyApps", user))
        && content.contains(&format!("file:///home/{}/WebRoots", user))
}

/// Inserts the snippet after the Desktop entry's closing `</bookmark>`; None when the marker is missing.
fn insert_dev_places(content: &str, user: &str) -> Option<String> {
    let title_marker = "<title>Desktop</title>";
    let title_pos = content.find(title_marker)?;
    let close_tag = "</bookmark>";
    let close_rel = content[title_pos..].find(close_tag)?;
    let insert_at = title_pos + close_rel + close_tag.len();
    let mut out = String::with_capacity(content.len() + 1024);
    out.push_str(&content[..insert_at]);
    out.push('\n');
    out.push_str(&dev_places_snippet(user));
    out.push_str(&content[insert_at..]);
    Some(out)
}

fn dev_places_added_probe(user: &str, home: &str) -> bool {
    std::fs::read_to_string(user_places_path(home))
        .map(|c| dev_places_present(&c, user))
        .unwrap_or(false)
}

/// Ensure ~/Coding + ~/MyApps + ~/WebRoots exist and carry Places
/// bookmarks. Shared by [Add Dev Folders] and the prerequisite flow.
/// Returns Ok(true) when the bookmarks were inserted, Ok(false) when
/// already present (folders are always created either way).
fn ensure_dev_folders(user: &str, home: &str) -> Result<bool, String> {
    // Bookmarks must resolve, so create all three target folders first.
    for dir in ["Coding", "MyApps", "WebRoots"] {
        let path = format!("{}/{}", home, dir);
        if std::fs::create_dir_all(&path).is_err() {
            return Err(format!("Could not create {} — check home directory permissions.", path));
        }
    }
    let places = user_places_path(home);
    let content = match std::fs::read_to_string(&places) {
        Ok(c) => c,
        Err(_) => {
            return Err(format!(
                "Could not read {} — is this a KDE Plasma session?",
                places
            ));
        }
    };
    if dev_places_present(&content, user) {
        return Ok(false);
    }
    let updated = insert_dev_places(&content, user).ok_or_else(|| {
        "Could not find the Desktop entry in user-places.xbel — Places file has an unexpected layout.".to_string()
    })?;
    if std::fs::write(&places, &updated).is_err() {
        return Err(format!("Could not write {} — check file permissions.", places));
    }
    Ok(true)
}

/// First available terminal emulator plus its run flag; gnome-terminal needs `--`.
fn terminal_emulator() -> Option<(&'static str, &'static str)> {
    for (bin, flag) in [
        ("konsole", "-e"),
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("xfce4-terminal", "-e"),
        ("xterm", "-e"),
    ] {
        // Binary names stay hardcoded; in-process PATH walk avoids a shell spawn.
        if crate::common::path_lookup(bin).is_some() {
            return Some((bin, flag));
        }
    }
    None
}

/// Opens `script_body` in a new terminal, detached so the caller never blocks.
fn launch_in_terminal(script_body: &str) -> Result<String, String> {
    let (bin, flag) = terminal_emulator()
        .ok_or_else(|| "No terminal emulator found (konsole / x-terminal-emulator / gnome-terminal / xterm).".to_string())?;
    Command::new(bin)
        .arg(flag)
        .arg("sh")
        .arg("-c")
        .arg(script_body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to open {}: {}", bin, e))?;
    Ok(bin.to_string())
}

/// SELinux off in /etc/selinux/config (Fedora prereq; nginx/cert tooling needs it).
fn selinux_disabled_in_config() -> bool {
    match std::fs::read_to_string("/etc/selinux/config") {
        Ok(t) => parse_selinux_config_disabled(&t),
        Err(_) => false,
    }
}

/// Pure parser: last effective SELINUX= must be `disabled`; SELINUXTYPE= never matches.
fn parse_selinux_config_disabled(text: &str) -> bool {
    let mut found = false;
    let mut disabled = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let bare = t.split(['#', ';']).next().unwrap_or("").trim();
        if let Some(v) = bare.strip_prefix("SELINUX=") {
            found = true;
            disabled = v.trim().to_lowercase() == "disabled";
        }
    }
    found && disabled
}

/// Green-LED rule: certutil everywhere, plus pkexec on Debian and SELINUX=disabled on Fedora.
fn prereq_satisfied(
    certutil: bool,
    pkexec: bool,
    fedora_like: bool,
    selinux_disabled: bool,
) -> bool {
    certutil && (fedora_like || pkexec) && (!fedora_like || selinux_disabled)
}

fn prereq_probe() -> (bool, bool, bool, String, String, bool) {
    let fedora = distro_is_fedora_like();
    let selinux_off = if fedora { selinux_disabled_in_config() } else { false };
    // Probe mirrors the install sets: certutil alone must not light the LED while pkexec is still missing.
    let ok = prereq_satisfied(
        certutil_present(),
        pkexec_installed(),
        fedora,
        selinux_off,
    );
    let in_sudo = in_sudo_group();
    let needs_sudo = distro_is_debian_like() && !in_sudo;
    match prereq_selection() {
        Some((label, pkgs, _)) => (
            ok,
            in_sudo,
            needs_sudo,
            pkgs.join(" "),
            label.to_string(),
            selinux_off,
        ),
        None => (ok, in_sudo, false, String::new(), String::new(), selinux_off),
    }
}

/// Missing base binaries (empty = installed); modules stay user-managed.
fn nginx_php_missing_bins() -> Vec<String> {
    let mut missing = Vec::new();
    if !binary_present("nginx", &["/usr/sbin/nginx", "/usr/local/sbin/nginx", "/usr/bin/nginx"]) {
        missing.push("nginx".to_string());
    }
    if !binary_present("php", &["/usr/bin/php", "/usr/local/bin/php"]) {
        missing.push("php".to_string());
    }
    let (fpm, paths): (&str, &[&str]) = if distro_is_debian_like() {
        ("php-fpm8.4", &["/usr/sbin/php-fpm8.4"])
    } else {
        ("php-fpm", &["/usr/sbin/php-fpm", "/usr/bin/php-fpm"])
    };
    if !binary_present(fpm, paths) {
        missing.push(fpm.to_string());
    }
    missing
}

/// Green iff the base binaries are present.
fn nginx_php_stack_installed() -> bool {
    nginx_php_missing_bins().is_empty()
}

/// Functional probe: asks polkit itself whether this user is authorized; true iff passwordless pkexec works.
fn pkexec_functional_active() -> bool {
    let pid = std::process::id().to_string();
    Command::new("pkcheck")
        .args(["-a", "org.freedesktop.policykit.exec", "-p", &pid])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True when the rule file carries our group marker plus a YES result.
fn rules_file_active(path: &str, group: &str) -> bool {
    let content = match read_rules_file(path) {
        Some(c) => c,
        None => return false,
    };
    content.contains(&format!("isInGroup(\"{}\")", group)) && content.contains("polkit.Result.YES")
}

fn sudoers_content(user: &str) -> String {
    format!("{} ALL=(ALL:ALL) NOPASSWD:ALL\n", user)
}

/// Fedora-only nginx conform steps for the Install script (the old
/// [Conform NGINX] button, now mandatory): timestamped backup + stable
/// .orig of stock nginx.conf, sites-available/sites-enabled dirs, default
/// site, rewritten nginx.conf with the dev user baked in (shared template),
/// default symlink, nginx -t. Runs BEFORE the service-user rewrite so the
/// backup captures pristine stock and the later `user` sed is a no-op.
/// Idempotent: existing .orig, default site and symlink are kept.
/// Debian already ships this layout and is never touched.
fn fedora_conform_fragment(user: &str) -> String {
    let conf = FEDORA_NGINX_CONF_TEMPLATE.replace("@@USER@@", user);
    format!(
        r###"cp -a /etc/nginx/nginx.conf /etc/nginx/nginx.conf.bak-$(date +%F-%H%M%S) && {{ [ -e /etc/nginx/nginx.conf.orig ] || cp -a /etc/nginx/nginx.conf /etc/nginx/nginx.conf.orig; }} && mkdir -p /etc/nginx/sites-available /etc/nginx/sites-enabled && chmod 755 /etc/nginx/sites-available /etc/nginx/sites-enabled && {{ [ -e /etc/nginx/sites-available/default ] || cat > /etc/nginx/sites-available/default << 'DUCKNET_DEFAULT_SITE_EOF'
{site}DUCKNET_DEFAULT_SITE_EOF
}} && chmod 644 /etc/nginx/sites-available/default && cat > /etc/nginx/nginx.conf << 'DUCKNET_NGINX_CONF_EOF'
{conf}DUCKNET_NGINX_CONF_EOF
chmod 644 /etc/nginx/nginx.conf && {{ [ -e /etc/nginx/sites-enabled/default ] || [ -L /etc/nginx/sites-enabled/default ] || ln -s ../sites-available/default /etc/nginx/sites-enabled/default; }} && nginx -t"###,
        site = DEBIAN_DEFAULT_SITE_CONTENT,
        conf = conf,
    )
}

/// Post-install service-user rewrite chained onto Install; no systemd override (it caused knock-on permission problems).
fn service_user_script(variant: &str, user: &str, group: &str) -> Option<String> {
    let (pool, fpm_service, php_ini) = match variant {
        "debian" => (
            "/etc/php/8.4/fpm/pool.d/www.conf",
            "php8.4-fpm.service",
            "/etc/php/8.4/cli/php.ini",
        ),
        "fedora" => (
            "/etc/php-fpm.d/www.conf",
            "php-fpm.service",
            "/etc/php.ini",
        ),
        _ => return None,
    };
    // Session dir stays user-owned (mkdir runs as root, so chown back); pool edits are replace-or-append because stock files ship them commented or absent.
    let home = home_dir_for(user);
    let session = format!("{}/WebRoots/.php-session", home);
    Some(format!(
        "sed -i \"s/^user = .*/user = {user}/\" {pool} && sed -i \"s/^group = .*/group = {group}/\" {pool} && sed -i \"s/^;*listen.owner =.*/listen.owner = {user}/\" {pool} && sed -i \"s/^;*listen.group =.*/listen.group = {group}/\" {pool} && sed -i \"s/^;*listen.mode =.*/listen.mode = 0660/\" {pool} && sed -i \"s/^;*listen.acl_users =.*/listen.acl_users = {user}/\" {pool} && sed -i -E \"s/^\\s*user\\s+.*;/user {user};/\" /etc/nginx/nginx.conf && mkdir -p {session} && chown {user}:{group} {home}/WebRoots {session} && chmod 700 {session} && sed -i -E \"s|^[; ]*php_value\\\\[session.save_path\\\\].*|php_value[session.save_path] = {session}|\" {pool} && grep -q \"^php_value\\\\[session.save_path\\\\]\" {pool} || echo \"php_value[session.save_path] = {session}\" >> {pool} && sed -i -E \"s|^[; ]*php_admin_value\\\\[memory_limit\\\\].*|php_admin_value[memory_limit] = 512M|\" {pool} && grep -q \"^php_admin_value\\\\[memory_limit\\\\]\" {pool} || echo \"php_admin_value[memory_limit] = 512M\" >> {pool} && sed -i -E \"s/^memory_limit.*/memory_limit = 512M/\" {php_ini} && systemctl daemon-reload && systemctl restart nginx.service {fpm_service}"
    ))
}
/// Polkit rule body; only the privileged group differs.
fn polkit_rule_content(group: &str) -> String {
    format!(
        "polkit.addRule(function(action, subject) {{\n    if (subject.local && subject.active && subject.isInGroup(\"{group}\")) {{\n        return polkit.Result.YES;\n    }}\n}});\n"
    )
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, sudo_nopasswd, cxx_name = "sudoNopasswd")]
        #[qproperty(bool, pkexec_nopasswd, cxx_name = "pkexecNopasswd")]
        #[qproperty(bool, pkexec_installed, cxx_name = "pkexecInstalled")]
        #[qproperty(QString, pkexec_variant, cxx_name = "pkexecVariant")]
        #[qproperty(bool, prereq_ok, cxx_name = "prereqOk")]
        #[qproperty(bool, in_sudo_group, cxx_name = "inSudoGroup")]
        #[qproperty(bool, needs_sudo_group, cxx_name = "needsSudoGroup")]
        #[qproperty(bool, selinux_off, cxx_name = "selinuxOff")]
        #[qproperty(QString, prereq_packages, cxx_name = "prereqPackages")]
        #[qproperty(QString, prereq_variant, cxx_name = "prereqVariant")]
        #[qproperty(bool, stack_installed, cxx_name = "stackInstalled")]
        #[qproperty(bool, op_active, cxx_name = "opActive")]
        #[qproperty(QString, op_log, cxx_name = "opLog")]
        #[qproperty(bool, op_ok, cxx_name = "opOk")]
        #[qproperty(QString, op_name, cxx_name = "opName")]
        #[qproperty(QString, distro_family, cxx_name = "distroFamily")]
        #[qproperty(QString, dev_user, cxx_name = "devUser")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        #[qproperty(bool, dev_places_added, cxx_name = "devPlacesAdded")]
        type ToolingManager = super::ToolingManagerRust;

        /// Re-probes both access states (unprivileged, never prompts).
        #[qinvokable]
        #[cxx_name = "refreshStatus"]
        fn refresh_status(self: Pin<&mut Self>) -> bool;

        /// Install /etc/sudoers.d/<user> (staged, visudo-validated).
        #[qinvokable]
        #[cxx_name = "enablePasswordlessSudo"]
        fn enable_passwordless_sudo(self: Pin<&mut Self>) -> bool;

        /// Removes /etc/sudoers.d/<user>; kills the cached timestamp so the change bites immediately.
        #[qinvokable]
        #[cxx_name = "disablePasswordlessSudo"]
        fn disable_passwordless_sudo(self: Pin<&mut Self>) -> bool;

        /// Installs the polkit rule for this distro (sudo on Debian, wheel on Fedora).
        #[qinvokable]
        #[cxx_name = "enablePasswordlessPkexec"]
        fn enable_passwordless_pkexec(self: Pin<&mut Self>) -> bool;

        /// Removes this distro's polkit rule and restarts polkitd so the removal bites now.
        #[qinvokable]
        #[cxx_name = "disablePasswordlessPkexec"]
        fn disable_passwordless_pkexec(self: Pin<&mut Self>) -> bool;

        /// Starts Install/Uninstall NGINX+PHP, DevEnv, VSCodium, Opencode in the background.
        /// Returns false while an op runs or the distro is unsupported; QML polls `pollPackageOp`.
        #[qinvokable]
        #[cxx_name = "startPackageOp"]
        fn start_package_op(self: Pin<&mut Self>, op: &QString) -> bool;

        /// Installs prerequisites in a new terminal (fire-and-forget); the LED follows on re-probe.
        /// Debian needs the sudo group first; `reboot` reboots on success instead of holding open.
        /// Also ensures the Coding/MyApps/WebRoots dev folders + Places bookmarks (best-effort).
        #[qinvokable]
        #[cxx_name = "installPrerequisites"]
        fn install_prerequisites(self: Pin<&mut Self>, reboot: bool) -> bool;

        /// Stages the sudo-enrollment script and opens it in a terminal; Debian-family only.
        #[qinvokable]
        #[cxx_name = "addUserToSudoGroup"]
        fn add_user_to_sudo_group(self: Pin<&mut Self>) -> bool;

        /// Polls the background op; cheap when idle (one mutex check plus a small file read).
        #[qinvokable]
        #[cxx_name = "pollPackageOp"]
        fn poll_package_op(self: Pin<&mut Self>) -> bool;

        /// Adds Coding + MyApps + WebRoots folders and Places bookmarks, then opens the file browser (unprivileged).
        #[qinvokable]
        #[cxx_name = "addDevFolders"]
        fn add_dev_folders(self: Pin<&mut Self>) -> bool;
    }
}

pub struct ToolingManagerRust {
    sudo_nopasswd: bool,
    pkexec_nopasswd: bool,
    pkexec_installed: bool,
    pkexec_variant: QString,
    prereq_ok: bool,
    in_sudo_group: bool,
    needs_sudo_group: bool,
    selinux_off: bool,
    prereq_packages: QString,
    prereq_variant: QString,
    stack_installed: bool,
    op_active: bool,
    op_log: QString,
    op_ok: bool,
    op_name: QString,
    distro_family: QString,
    dev_user: QString,
    status_message: QString,
    dev_places_added: bool,
}

impl Default for ToolingManagerRust {
    fn default() -> Self {
        Self {
            sudo_nopasswd: false,
            pkexec_nopasswd: false,
            pkexec_installed: false,
            pkexec_variant: QString::default(),
            prereq_ok: false,
            in_sudo_group: false,
            needs_sudo_group: false,
            selinux_off: false,
            prereq_packages: QString::default(),
            prereq_variant: QString::default(),
            stack_installed: false,
            op_active: false,
            op_log: QString::default(),
            op_ok: false,
            op_name: QString::default(),
            distro_family: QString::default(),
            dev_user: QString::default(),
            status_message: QString::default(),
            dev_places_added: false,
        }
    }
}

impl qobject::ToolingManager {
    /// Runs a privileged command and returns (success, output).
    fn priv_step(argv: &[&str]) -> Result<String, String> {
        match privileged_output(argv) {
            Ok(out) => {
                let combined = format!(
                    "{} {}",
                    String::from_utf8_lossy(&out.stdout).trim(),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                if out.status.success() {
                    Ok(combined.trim().to_string())
                } else {
                    Err(combined.trim().to_string())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// (rules file, group, label) for this distro; unknown distros get None.
    fn pkexec_selection() -> Option<(&'static str, &'static str, &'static str)> {
        if distro_is_debian_like() {
            Some((DEBIAN_RULES_FILE, "sudo", "Debian"))
        } else if distro_is_fedora_like() {
            Some((FEDORA_RULES_FILE, "wheel", "Fedora"))
        } else {
            None
        }
    }

    /// Collects all probe values; setters live with callers (cxx-qt mutates via Pin only).
    fn probe_all() -> (bool, bool, bool, bool, String, String, String) {
        let sudo = sudo_nopasswd_active();
        let present = pkexec_installed();
        let stack = nginx_php_stack_installed();
        let (pkexec, variant) = match Self::pkexec_selection() {
            // Active on our variant's rule or on polkit's own authorization (covers unreadable rules.d and third-party rules).
            Some((file, group, label)) => (
                rules_file_active(file, group) || pkexec_functional_active(),
                label.to_string(),
            ),
            None => (pkexec_functional_active(), String::new()),
        };
        (sudo, pkexec, present, stack, variant, distro_id(), dev_username())
    }

    fn apply_probe(mut self: Pin<&mut Self>) {
        let (sudo, pkexec, present, stack, variant, distro, dev) = Self::probe_all();
        self.as_mut().set_sudo_nopasswd(sudo);
        self.as_mut().set_pkexec_nopasswd(pkexec);
        self.as_mut().set_pkexec_installed(present);
        self.as_mut().set_stack_installed(stack);
        self.as_mut().set_pkexec_variant(QString::from(&variant));
        self.as_mut().set_distro_family(QString::from(&distro));
        self.as_mut().set_dev_user(QString::from(&dev));
        self.as_mut().apply_prereq_probe();
    }

    /// Pushes prereq + dev-places probes; the idle poller uses it so LEDs follow external changes.
    fn apply_prereq_probe(mut self: Pin<&mut Self>) {
        let (ok, in_sudo, needs_sudo, pkgs, variant, selinux_off) = prereq_probe();
        self.as_mut().set_prereq_ok(ok);
        self.as_mut().set_in_sudo_group(in_sudo);
        self.as_mut().set_needs_sudo_group(needs_sudo);
        self.as_mut().set_selinux_off(selinux_off);
        self.as_mut().set_prereq_packages(QString::from(&pkgs));
        self.as_mut().set_prereq_variant(QString::from(&variant));
        let user = dev_username();
        let home = home_dir_for(&user);
        self.as_mut()
            .set_dev_places_added(dev_places_added_probe(&user, &home));
    }

    fn refresh_status(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().apply_probe();
        log_debug(&format!(
            "[ducknet] tooling status: sudo_nopasswd={} pkexec_nopasswd={} pkexec_present={} stack_installed={} stack_missing_bins={:?} ({} rule) prereq_ok={} in_sudo={} needs_sudo={} prereq_pkgs='{}' distro='{}'",
            self.sudo_nopasswd, self.pkexec_nopasswd, self.pkexec_installed, self.stack_installed, nginx_php_missing_bins(), self.pkexec_variant, self.prereq_ok, self.in_sudo_group, self.needs_sudo_group, self.prereq_packages, self.distro_family,
        ));
        true
    }

    fn disable_passwordless_sudo(mut self: Pin<&mut Self>) -> bool {
        let user = dev_username();
        if !valid_username(&user) {
            self.as_mut()
                .set_status_message(QString::from("Refusing: unexpected username."));
            return false;
        }
        let dest = format!("/etc/sudoers.d/{}", user);
        // Best effort when already absent; the end state already holds.
        if std::path::Path::new(&dest).exists() {
            if Self::priv_step(&["rm", "-f", &dest]).is_err() {
                let msg = format!("Sudoers removal failed {NEEDS_ROOT}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        // A cached timestamp also satisfies `sudo -n true`; kill it so disable bites immediately.
        let _ = Command::new("sudo").arg("-n").arg("-K").output();
        self.as_mut().apply_probe();
        let msg = if self.sudo_nopasswd {
            format!("Entry for {} removed but sudo -n still succeeds (another rule grants it).", user)
        } else {
            format!("Passwordless sudo disabled for {} (console).", user)
        };
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        !self.sudo_nopasswd
    }

    /// Stages content in /tmp, then installs it as root.
    fn stage_and_install(staged: &std::path::Path, dest: &str, mode: &str) -> Result<(), String> {
        Self::priv_step(&["cp", &staged.to_string_lossy(), dest])
            .map_err(|e| format!("install failed (needs root): {}", e))?;
        Self::priv_step(&["chmod", mode, dest])
            .map_err(|e| format!("chmod {} failed: {}", mode, e))?;
        Ok(())
    }

    fn enable_passwordless_sudo(mut self: Pin<&mut Self>) -> bool {
        if sudo_nopasswd_active() {
            self.as_mut()
                .set_status_message(QString::from("Passwordless sudo already active."));
            return true;
        }
        let user = dev_username();
        if !valid_username(&user) {
            self.as_mut()
                .set_status_message(QString::from("Refusing: unexpected username."));
            return false;
        }
        let dest = format!("/etc/sudoers.d/{}", user);
        let staged = std::env::temp_dir().join(format!("ducknet-sudoers-{}", user));
        if std::fs::write(&staged, sudoers_content(&user)).is_err() {
            self.as_mut()
                .set_status_message(QString::from("Failed to stage sudoers file."));
            return false;
        }
        if Self::stage_and_install(&staged, &dest, "0440").is_err() {
            let msg = format!("Sudo install failed {NEEDS_ROOT}");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if Self::priv_step(&["chown", "root:root", &dest]).is_err() {
            let _ = Self::priv_step(&["rm", "-f", &dest]);
            let msg = "Sudo install failed at chown — removed, system untouched.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        // Safety gate: a broken sudoers file locks the user out, so validate before declaring success.
        if Self::priv_step(&["visudo", "-c", "-f", &dest]).is_err() {
            let _ = Self::priv_step(&["rm", "-f", &dest]);
            let msg = "visudo rejected the sudoers file — removed, system untouched.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(&staged);
        self.as_mut().apply_probe();
        let msg = if self.sudo_nopasswd {
            format!("Passwordless sudo enabled for {} (console).", user)
        } else {
            format!("Sudoers file installed for {} but sudo -n still fails — check group/policy.", user)
        };
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        self.sudo_nopasswd
    }

    fn enable_passwordless_pkexec(mut self: Pin<&mut Self>) -> bool {
        let (rules_file, group, label) = match Self::pkexec_selection() {
            Some(s) => s,
            None => {
                let msg = format!(
                    "Passwordless pkexec is only wired for Debian/Fedora-family — this system is '{}'.",
                    distro_id()
                );
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        if rules_file_active(rules_file, group) {
            let msg = format!("{} passwordless pkexec already active.", label);
            self.as_mut().set_status_message(QString::from(&msg));
            return true;
        }
        // Default Debian 13 Plasma ships without pkexec, so install it first.
        if !pkexec_installed() {
            if label != "Debian" {
                let msg = "pkexec not found — install the polkit package first (sudo dnf install polkit).".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            log_debug("[ducknet] pkexec missing — installing via apt-get");
            if Self::priv_step(&["apt-get", "install", "-y", "pkexec"]).is_err()
                || !pkexec_installed()
            {
                let msg = "pkexec is not installed and automatic install needs root — run 'sudo apt install pkexec' in a terminal, then retry.".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        let staged = std::env::temp_dir().join(format!("ducknet-polkit-{}.rules", group));
        if std::fs::write(&staged, polkit_rule_content(group)).is_err() {
            self.as_mut()
                .set_status_message(QString::from("Failed to stage polkit rule."));
            return false;
        }
        if Self::stage_and_install(&staged, rules_file, "644").is_err() {
            let msg = format!("Polkit rule install failed {NEEDS_ROOT}");
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(&staged);
        // polkitd picks up new rules only on restart.
        let polkit_ok = Self::priv_step(&["systemctl", "restart", "polkit"]).is_ok();
        self.as_mut().apply_probe();
        let msg = if polkit_ok {
            format!("{} passwordless pkexec enabled — desktop prompts will no longer ask for a password.", label)
        } else {
            format!("{} rule installed but polkit restart failed — log out/in or reboot to apply.", label)
        };
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn disable_passwordless_pkexec(mut self: Pin<&mut Self>) -> bool {
        let (rules_file, group, label) = match Self::pkexec_selection() {
            Some(s) => s,
            None => {
                let msg = format!(
                    "Passwordless pkexec is only wired for Debian/Fedora-family — this system is '{}'.",
                    distro_id()
                );
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        // Best effort when already absent; the end state already holds.
        if std::path::Path::new(rules_file).exists() {
            if Self::priv_step(&["rm", "-f", rules_file]).is_err() {
                let msg = format!("Polkit rule removal failed {NEEDS_ROOT}");
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        }
        let _ = Self::priv_step(&["systemctl", "restart", "polkit"]);
        self.as_mut().apply_probe();
        let msg = if rules_file_active(rules_file, group) {
            format!("{} rule file still present — removal did not take effect.", label)
        } else if self.pkexec_nopasswd {
            format!("{} rule removed, but pkexec is still passwordless via another rule.", label)
        } else {
            format!("{} passwordless pkexec disabled.", label)
        };
        let ok = !rules_file_active(rules_file, group);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        ok
    }

    /// Opens the prerequisite install in a terminal; the idle poller flips the LED without a manual refresh.
    fn install_prerequisites(mut self: Pin<&mut Self>, reboot: bool) -> bool {
        let (label, pkgs, _) = match prereq_selection() {
            Some(s) => s,
            None => {
                let msg = format!(
                    "Prerequisites are only wired for Debian/Fedora-family — this system is '{}'.",
                    distro_id()
                );
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        // Early-out mirrors the LED gate; certutil alone once stranded both distros.
        let (satisfied, _, _, _, _, selinux_off) = prereq_probe();
        if satisfied {
            let msg = "Prerequisites already satisfied.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_prereq_probe();
            return true;
        }
        let variant = if distro_is_debian_like() { "debian" } else { "fedora" };
        // Never reboot an already-compliant machine; the shell re-checks at runtime too.
        let effective_reboot = reboot && !(variant == "fedora" && selinux_off);
        // Dev folders ride along with prerequisites (same helper as the
        // [Add Dev Folders] button). Best-effort: a non-Plasma session
        // without user-places.xbel must never block the package install.
        let user = dev_username();
        let mut folders_note = String::new();
        if valid_username(&user) {
            match ensure_dev_folders(&user, &home_dir_for(&user)) {
                Ok(_) => {
                    self.as_mut().set_dev_places_added(true);
                    folders_note = " Dev folders ensured.".to_string();
                }
                Err(e) => log_debug(&format!("[ducknet] dev folders skipped: {}", e)),
            }
        }
        let script = match prereq_install_script(variant, pkgs, effective_reboot) {
            Some(s) => s,
            None => {
                self.as_mut()
                    .set_status_message(QString::from("Unknown prerequisite operation."));
                return false;
            }
        };
        match launch_in_terminal(&script) {
            Ok(emulator) => {
                let mut msg = format!(
                    "{} prerequisites installing in {} — enter your password there when asked. The LED turns green once certutil is detected",
                    label, emulator
                );
                if label == "Fedora" {
                    msg.push_str(" and SELinux is disabled in /etc/selinux/config (disabled live now, permanent after a reboot)");
                }
                if effective_reboot {
                    msg.push_str(". The system will reboot immediately afterwards.");
                } else if reboot {
                    msg.push_str(". SELinux was already disabled — no reboot needed.");
                } else {
                    msg.push('.');
                }
                msg.push_str(&folders_note);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("Could not open a terminal: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    /// Stages the sudo-enrollment script and opens it in a terminal; reboots immediately (group needs a fresh login).
    fn add_user_to_sudo_group(mut self: Pin<&mut Self>) -> bool {
        if !distro_is_debian_like() {
            let msg = format!(
                "The sudo-group step is Debian-family only — this system is '{}' (Fedora uses the wheel group).",
                distro_id()
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let user = dev_username();
        if !valid_username(&user) {
            self.as_mut()
                .set_status_message(QString::from("Refusing: unexpected username."));
            return false;
        }
        if in_sudo_group() {
            let msg = format!("{} is already in the sudo group.", user);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_prereq_probe();
            return true;
        }
        if std::fs::write(FIXSUDO_STAGED_PATH, fixsudo_script_content(&user)).is_err() {
            self.as_mut()
                .set_status_message(QString::from("Failed to stage the sudo script."));
            return false;
        }
        // Owner-executable only; the script prompts for the root password itself.
        let _ = Command::new("chmod").arg("700").arg(FIXSUDO_STAGED_PATH).output();
        let body = format!("{}; exec sh", FIXSUDO_STAGED_PATH);
        match launch_in_terminal(&body) {
            Ok(_) => {
                let msg = "Terminal opened — enter the ROOT password there. The system will reboot immediately afterwards.".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("Could not open a terminal: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    /// Creates ~/Coding + ~/MyApps + ~/WebRoots and inserts all three Places bookmarks (already-present counts as success).
    fn add_dev_folders(mut self: Pin<&mut Self>) -> bool {
        let user = dev_username();
        if !valid_username(&user) {
            self.as_mut()
                .set_status_message(QString::from("Refusing: unexpected username."));
            return false;
        }
        let home = home_dir_for(&user);
        match ensure_dev_folders(&user, &home) {
            Ok(changed) => {
                self.as_mut().set_dev_places_added(true);
                let msg = if changed {
                    "Coding, MyApps and WebRoots added to Places.".to_string()
                } else {
                    "Dev folders already in Places — nothing to do.".to_string()
                };
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                Self::open_file_browser(&home);
                true
            }
            Err(msg) => {
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    /// Opens the default file browser (fire-and-forget); failures stay best-effort.
    fn open_file_browser(dir: &str) {
        match Command::new("xdg-open")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => log_debug(&format!("[ducknet] opened file browser on {}", dir)),
            Err(e) => log_debug(&format!("[ducknet] failed to open file browser: {}", e)),
        }
    }

    fn start_package_op(mut self: Pin<&mut Self>, op: &QString) -> bool {        let op = op.to_string();
        if op != "install" && op != "uninstall" && op != "devenv" && op != "vscodium" && op != "opencode" {
            self.as_mut()
                .set_status_message(QString::from("Unknown package operation."));
            return false;
        }
        if let Ok(guard) = PKG_OP.lock() {
            if let Some(st) = guard.as_ref() {
                if !st.done {
                    let msg = format!("{} is already running.", st.title);
                    self.as_mut().set_status_message(QString::from(&msg));
                    return false;
                }
            }
        }
        let variant = if distro_is_debian_like() {
            "debian"
        } else if distro_is_fedora_like() {
            "fedora"
        } else {
            let msg = format!(
                "Package install is only wired for Debian/Fedora-family — this system is '{}'.",
                distro_id()
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        };
        let user = dev_username();
        if (op == "devenv" || op == "opencode") && !valid_username(&user) {
            self.as_mut()
                .set_status_message(QString::from("Refusing: unexpected username."));
            return false;
        }
        let argv = match package_command(variant, &op, &user) {
            Some(a) => a,
            None => {
                self.as_mut()
                    .set_status_message(QString::from("Unknown package operation."));
                return false;
            }
        };
        let title = match op.as_str() {
            "install" => "Install NGINX + PHP",
            "uninstall" => "Uninstall NGINX + PHP",
            "vscodium" => "Install VSCodium",
            "opencode" => "Install Opencode",
            _ => "Install Dev Env",
        }
        .to_string();
        let header = format!("DuckNet Dev Tool: {} ({})\n$ {}\n\n", title, variant, argv.join(" "));
        if std::fs::write(PKG_OP_LOG, &header).is_err() {
            self.as_mut()
                .set_status_message(QString::from("Failed to open package log."));
            return false;
        }
        match privileged_spawn(&argv, PKG_OP_LOG) {
            Ok(child) => {
                if let Ok(mut guard) = PKG_OP.lock() {
                    *guard = Some(PkgOp { child, title: title.clone(), done: false, ok: false, code: None });
                }
                self.as_mut().set_op_active(true);
                self.as_mut().set_op_name(QString::from(&title));
                self.as_mut().set_op_log(QString::from(&header));
                self.as_mut().set_op_ok(false);
                log_debug(&format!("[ducknet] {} started: {}", title, argv.join(" ")));
                true
            }
            Err(e) => {
                let msg = format!("{} {NEEDS_ROOT}: {}", title, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    fn poll_package_op(mut self: Pin<&mut Self>) -> bool {
        let (active, just_finished, ok, code) = match PKG_OP.lock() {
            Ok(mut guard) => match guard.as_mut() {
                None => (false, false, false, None),
                Some(st) => {
                    if !st.done {
                        match st.child.try_wait() {
                            Ok(Some(status)) => {
                                st.done = true;
                                st.ok = status.success();
                                st.code = status.code();
                                (false, true, st.ok, st.code)
                            }
                            Ok(None) => (true, false, false, None),
                            Err(_) => {
                                st.done = true;
                                st.ok = false;
                                (false, true, false, None)
                            }
                        }
                    } else {
                        (false, false, st.ok, st.code)
                    }
                }
            },
            Err(_) => (false, false, false, None),
        };
        self.as_mut().set_op_active(active);
        self.as_mut().set_op_log(QString::from(&op_log_tail(PKG_OP_LOG)));
        // Idle poll re-probes so the LED flips green after a terminal install finishes.
        if !active {
            self.as_mut().apply_prereq_probe();
        }
        if just_finished {
            self.as_mut().set_op_ok(ok);
            // Re-probe so the LED follows without a manual refresh; log the missing set for explainability.
            let missing = nginx_php_missing_bins();
            self.as_mut().set_stack_installed(missing.is_empty());
            let title = self.op_name.to_string();
            let title = if title.is_empty() { "Package operation".to_string() } else { title };
            let msg = if ok {
                format!("{} finished successfully.", title)
            } else {
                format!("{} failed{}. See {} for details.",
                    title,
                    code.map(|c| format!(" (exit {})", c)).unwrap_or_default(),
                    PKG_OP_LOG)
            };
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] {} stack_missing={:?}", msg, missing));
        }
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Test helpers live in common.
    use crate::common::{parse_os_release_id, whereis_has_binary};

    /// Neutral fixture username; never the machine's real dev user.
    const TEST_USER: &str = "testdev";

    #[test]
    fn username_validation() {
        assert!(valid_username(TEST_USER));
        assert!(valid_username("dev-user_2"));
        assert!(!valid_username(""));
        assert!(!valid_username("root"));
        assert!(!valid_username("admin/user"));
        assert!(!valid_username("ad min"));
        assert!(!valid_username("../../etc"));
        assert!(!valid_username("Admin")); // must start lowercase/underscore
        assert!(!valid_username("9lives"));
    }

    #[test]
    fn os_release_id_parsing() {
        assert_eq!(parse_os_release_id("ID=debian\nVERSION_ID=\"13\"\n"), "debian");
        assert_eq!(parse_os_release_id("ID=\"fedora\"\n"), "fedora");
        assert_eq!(parse_os_release_id("NAME=Whatever\n"), "");
        assert_eq!(parse_os_release_id(""), "");
    }

    #[test]
    fn sudoers_content_render() {
        assert_eq!(
            sudoers_content(TEST_USER),
            format!("{} ALL=(ALL:ALL) NOPASSWD:ALL\n", TEST_USER)
        );
    }

    #[test]
    fn polkit_rule_targets_group() {
        let deb = polkit_rule_content("sudo");
        assert!(deb.contains("isInGroup(\"sudo\")"));
        assert!(deb.contains("polkit.Result.YES"));
        assert!(!deb.contains("wheel"));
        let fed = polkit_rule_content("wheel");
        assert!(fed.contains("isInGroup(\"wheel\")"));
    }

    #[test]
    fn rules_file_read_direct_and_missing() {
        // Direct read covers world-readable files.

        let dir = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("ducknet-test-readable-{}", stamp));
        let ps = path.to_string_lossy().to_string();
        std::fs::write(&path, polkit_rule_content("sudo")).unwrap();
        let read = read_rules_file(&ps);
        assert!(read.is_some());
        assert!(read.unwrap().contains("isInGroup(\"sudo\")"));
        let _ = std::fs::remove_file(&path);
        // Missing file reads as None.
        assert!(read_rules_file(&format!("{}-missing", ps)).is_none());
    }

    #[test]
    fn dpkg_status_parsing() {
        assert!(dpkg_status_installed("Package: pkexec\nStatus: install ok installed\nPriority: optional\n"));
        assert!(!dpkg_status_installed("Package: pkexec\nStatus: deinstall ok config-files\n"));
        assert!(!dpkg_status_installed("dpkg-query: package 'pkexec' is not installed\n"));
        assert!(!dpkg_status_installed(""));
    }

    #[test]
    fn package_commands_cover_all_combos() {
        // Debian install covers the exact package set.
        let di = package_command("debian", "install", TEST_USER).unwrap();
        assert_eq!(&di[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(di[2].contains("apt-get update && apt-get install -y"));
        for p in ["nginx-full", "php8.4", "php8.4-fpm", "php-pear"] {
            assert!(di[2].contains(p), "missing {}", p);
        }
        assert!(di[2].contains("DEBIAN_FRONTEND=noninteractive"));
        assert!(di[2].contains("/etc/php/8.4/fpm/pool.d/www.conf"));
        assert!(di[2].contains(&format!("user {};", TEST_USER)));
        assert!(di[2].contains("systemctl restart nginx.service php8.4-fpm.service"));
        // Debian uninstall removes without purging (keeps configs).
        let du = package_command("debian", "uninstall", TEST_USER).unwrap();
        assert!(du[2].contains("apt-get remove -y"));
        assert!(du[2].contains("nginx-full"));
        assert!(!du[2].contains("purge"));
        // Fedora install leads with the Remi repo plus the php:remi-8.4 switch.
        let fi = package_command("fedora", "install", TEST_USER).unwrap();
        assert_eq!(&fi[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(fi[2].contains("dnf install -y"));
        assert!(fi[2].contains("nginx-all-modules"));
        assert!(fi[2].contains("php-pecl-imagick-im7"));
        // Generic name would pull Fedora's conflicting 8.5-linked build.
        assert!(!fi[2].contains(" php-pecl-imagick "));
        assert!(!fi[2].contains("php84-php-pecl-imagick"));
        assert!(fi[2].contains("/etc/php-fpm.d/www.conf"));
        assert!(fi[2].contains("systemctl restart nginx.service php-fpm.service"));
        // Remi flow stays ordered: repo, verification, module switch, services.
        for step in [
            "rpms.remirepo.net/fedora/remi-release-$(rpm -E %fedora).rpm",
            "rpm -q remi-release",
            "dnf repo list --enabled | grep -E '^remi'",
            "rpm -ql remi-release | grep -E '/etc/yum.repos.d/|/etc/pki/rpm-gpg/'",
            "dnf makecache",
            "dnf module reset php",
            "dnf module enable php:remi-8.4 -y",
            "systemctl enable --now nginx.service",
            "systemctl enable --now php-fpm.service",
        ] {
            assert!(fi[2].contains(step), "missing step {}", step);
        }
        // Repo setup precedes install; services go last.
        let pos = |s: &str| fi[2].find(s).unwrap();
        assert!(pos("remi-release-$") < pos("dnf install -y nginx"));
        assert!(pos("php:remi-8.4") < pos("dnf install -y nginx"));
        assert!(pos("systemctl enable --now php-fpm") > pos("systemctl restart nginx"));
        let fu = package_command("fedora", "uninstall", TEST_USER).unwrap();
        assert_eq!(&fu[0..3], &["dnf".to_string(), "remove".to_string(), "-y".to_string()]);
        // Uninstall removes exactly what install adds.
        for p in FEDORA_NGINX_PHP_PKGS {
            assert!(fi[2].contains(p), "install missing {}", p);
            assert!(fu.contains(&p.to_string()), "uninstall missing {}", p);
        }
        // Unknown combos refuse.
        assert!(package_command("arch", "install", TEST_USER).is_none());
        assert!(package_command("debian", "purge", TEST_USER).is_none());
        assert!(package_command("debian", "devenv", TEST_USER).is_some());
        assert!(package_command("arch", "vscodium", TEST_USER).is_none());
        assert!(package_command("arch", "opencode", TEST_USER).is_none());
    }

    #[test]
    fn fedora_install_conforms_before_service_rewrite() {
        // Conform markers present: backup, dirs, default site, rewritten
        // conf, symlink, config test.
        let fi = package_command("fedora", "install", TEST_USER).unwrap();
        for marker in [
            "nginx.conf.bak-",
            "nginx.conf.orig",
            "sites-available",
            "DUCKNET_DEFAULT_SITE_EOF",
            "DUCKNET_NGINX_CONF_EOF",
            "sites-enabled/default",
            "nginx -t",
        ] {
            assert!(fi[2].contains(marker), "missing {}", marker);
        }
        // Dev user baked into the conformed conf; no placeholder leaks.
        assert!(fi[2].contains(&format!("user {};", TEST_USER)));
        assert!(!fi[2].contains("@@USER@@"));
        // Ordering: packages -> conform (backup first) -> pool rewrite -> restarts.
        let pos = |s: &str| fi[2].find(s).unwrap();
        assert!(pos("dnf install -y nginx") < pos("nginx.conf.bak-"));
        assert!(pos("nginx.conf.bak-") < pos("/etc/php-fpm.d/www.conf"));
        assert!(pos("/etc/php-fpm.d/www.conf") < pos("systemctl restart nginx"));
        // Debian ships the layout already — never touched by conform.
        let di = package_command("debian", "install", TEST_USER).unwrap();
        assert!(!di[2].contains("sites-available"));
        assert!(!di[2].contains("nginx.conf.orig"));
        assert!(!di[2].contains("DUCKNET_NGINX_CONF_EOF"));
    }

    #[test]
    fn vscodium_commands_add_repo_then_install_codium() {
        // Debian covers keyring plus deb822 sources.
        let dv = package_command("debian", "vscodium", TEST_USER).unwrap();
        assert_eq!(&dv[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(dv[2].contains(VSCODIUM_GPG_URL));
        assert!(dv[2].contains("gpg --dearmor"));
        assert!(dv[2].contains(VSCODIUM_DEB_KEYRING));
        assert!(dv[2].contains(VSCODIUM_DEB_SOURCES));
        assert!(dv[2].contains("https://download.vscodium.com/debs"));
        assert!(dv[2].contains("apt-get update && apt-get install -y codium"));
        assert!(dv[2].contains("DEBIAN_FRONTEND=noninteractive"));
        // Fedora overwrites the .repo file (stays idempotent).
        let fv = package_command("fedora", "vscodium", TEST_USER).unwrap();
        assert_eq!(&fv[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(fv[2].contains(VSCODIUM_RPM_REPO));
        assert!(fv[2].contains("gitlab.com_paulcarroty_vscodium_repo"));
        assert!(fv[2].contains("https://paulcarroty.gitlab.io/vscodium-deb-rpm-repo/rpms/"));
        assert!(fv[2].contains(VSCODIUM_GPG_URL));
        assert!(fv[2].contains("dnf install -y codium"));
        assert!(fv[2].contains(&format!("> {}", VSCODIUM_RPM_REPO)));
        // Root shell takes no sudo prefix and no username interpolation.
        for script in [&dv[2], &fv[2]] {
            assert!(!script.contains("sudo"), "must not sudo inside root shell: {}", script);
        }
        assert!(!dv[2].contains(TEST_USER));
        assert!(!fv[2].contains(TEST_USER));
        // `sh -n` parses without executing.
        for (v, o) in [("debian", "vscodium"), ("fedora", "vscodium")] {
            let argv = package_command(v, o, TEST_USER).unwrap();
            let script = argv.last().unwrap();
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", script])
                .status()
                .expect("sh must exist for syntax check");
            assert!(status.success(), "shell syntax invalid for {}/{}: {}", v, o, script);
        }
    }

    #[test]
    fn opencode_command_is_universal_and_userspaced() {
        // Identical pipe on both distros (universal install).
        let dd = package_command("debian", "opencode", TEST_USER).unwrap();
        let fd = package_command("fedora", "opencode", TEST_USER).unwrap();
        assert_eq!(&dd[0..2], &["sh".to_string(), "-c".to_string()]);
        assert_eq!(dd[2], fd[2]);
        assert!(dd[2].contains("https://opencode.ai/install | bash"));
        // Runs as the dev user so the CLI lands in ~/.opencode, not /root.
        assert!(dd[2].contains(&format!("sudo -u {}", TEST_USER)));
        assert!(!dd[2].contains("sudo -u root"));
        assert!(opencode_command(TEST_USER)[2].contains(OPENCODE_PIPE));
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", dd.last().unwrap()])
            .status()
            .expect("sh must exist for syntax check");
        assert!(status.success(), "shell syntax invalid: {}", dd[2]);
    }

    #[test]
    fn devenv_commands_chain_packages_then_rustup() {
        // Debian covers toolchain plus rustup as the dev user.
        let dd = package_command("debian", "devenv", TEST_USER).unwrap();
        assert_eq!(&dd[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(dd[2].contains("apt-get update && apt-get install -y"));
        for p in ["build-essential", "python3-venv", "clangd", "clang-format", "ninja-build", "manpages-dev", "g++"] {
            assert!(dd[2].contains(p), "missing {}", p);
        }
        assert!(dd[2].contains(&format!("sudo -u {}", TEST_USER)));
        assert!(dd[2].contains("https://sh.rustup.rs | sh -s -- -y"));
        assert!(!dd[2].contains("sudo -u root"));
        // Fedora covers dnf groups plus rustup as the dev user.
        let fd = package_command("fedora", "devenv", TEST_USER).unwrap();
        assert!(fd[2].contains("dnf install -y"));
        for p in ["@c-development", "@development-tools", "python3-devel", "clang-tools-extra", "man-pages", "gcc-c++"] {
            assert!(fd[2].contains(p), "missing {}", p);
        }
        assert!(fd[2].contains(&format!("sudo -u {}", TEST_USER)));
        assert!(fd[2].contains("https://sh.rustup.rs | sh -s -- -y"));
        // Hostile names stay rejected upstream and never reach the shell.
        assert!(!valid_username("a; rm -rf /"));
        assert!(!valid_username("u$(id)"));
        // `sh -n` parses without executing.
        for (v, o) in [("debian", "devenv"), ("fedora", "devenv"), ("debian", "install"), ("fedora", "install")] {
            let argv = package_command(v, o, TEST_USER).unwrap();
            let script = argv.last().unwrap();
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", script])
                .status()
                .expect("sh must exist for syntax check");
            assert!(status.success(), "shell syntax invalid for {}/{}: {}", v, o, script);
        }
    }

    #[test]
    fn whereis_binary_parsing() {
        // Man-only and empty outputs never match.
        assert!(whereis_has_binary("sh: /bin/sh /usr/share/man/man1/sh.1.gz", "sh"));
        assert!(!whereis_has_binary("sh: /usr/share/man/man1/sh.1.gz", "sh"));
        assert!(!whereis_has_binary("php-fpm:", "php-fpm"));
        assert!(!whereis_has_binary("", "nginx"));
        // Man pages never satisfy the binary name.
        assert!(!whereis_has_binary("sh: /usr/share/man/man1/sh.1.gz", "sh"));
        assert!(whereis_has_binary(
            "nginx: /usr/sbin/nginx /usr/lib/nginx /etc/nginx /usr/share/man/man8/nginx.8.gz",
            "nginx"
        ));
    }

    #[test]
    fn stack_detected_on_this_host() {
        // Host fixture: nginx plus php plus php-fpm8.4.
        assert!(nginx_php_stack_installed());
        assert!(nginx_php_missing_bins().is_empty());
    }

    #[test]
    fn service_user_script_renders_per_distro() {
        let deb = service_user_script("debian", TEST_USER, TEST_USER).unwrap();
        assert!(deb.contains(&format!("s/^user = .*/user = {}/", TEST_USER)));
        assert!(deb.contains(&format!("s/^group = .*/group = {}/", TEST_USER)));
        assert!(deb.contains("/etc/php/8.4/fpm/pool.d/www.conf"));
        assert!(deb.contains(&format!("s/^\\s*user\\s+.*;/user {};/", TEST_USER)));
        assert!(deb.contains("systemctl restart nginx.service php8.4-fpm.service"));
        // No systemd override; it caused knock-on permission problems.
        assert!(!deb.contains("mkdir -p /etc/systemd/system/php8.4-fpm.service.d"));
        assert!(!deb.contains("/etc/systemd/system/php8.4-fpm.service.d/override.conf"));
        assert!(!deb.contains("override.conf"));
        assert!(!deb.contains(&format!("User={}", TEST_USER)));
        assert!(!deb.contains(&format!("Group={}", TEST_USER)));
        assert!(!deb.contains(&format!("chown -R {0}:{0} /var/lib/php/sessions", TEST_USER)));
        assert!(!deb.contains("chmod -R 770 /var/lib/php/sessions"));
        assert!(!deb.contains("/var/lib/php/sessions"));
        assert!(deb.contains(&format!("listen.owner = {}", TEST_USER)));
        assert!(deb.contains(&format!("listen.group = {}", TEST_USER)));
        assert!(deb.contains("listen.mode = 0660"));
        assert!(deb.contains(&format!("listen.acl_users = {}", TEST_USER)));
        assert!(deb.contains("systemctl daemon-reload"));
        let fed = service_user_script("fedora", "devuser", "devuser").unwrap();
        assert!(fed.contains("/etc/php-fpm.d/www.conf"));
        assert!(fed.contains("s/^\\s*user\\s+.*;/user devuser;/"));
        assert!(fed.contains("systemctl restart nginx.service php-fpm.service"));
        assert!(!fed.contains("mkdir -p /etc/systemd/system/php-fpm.service.d"));
        assert!(!fed.contains("/etc/systemd/system/php-fpm.service.d/override.conf"));
        assert!(!fed.contains("override.conf"));
        assert!(!fed.contains("User=devuser"));
        assert!(!fed.contains("Group=devuser"));
        assert!(!fed.contains("chown -R devuser:devuser /var/lib/php/session"));
        assert!(!fed.contains("chmod -R 770 /var/lib/php/session"));
        assert!(!fed.contains("/var/lib/php/session"));
        assert!(fed.contains("listen.owner = devuser"));
        assert!(fed.contains("listen.group = devuser"));
        assert!(fed.contains("listen.mode = 0660"));
        assert!(fed.contains("listen.acl_users = devuser"));
        assert!(fed.contains("systemctl daemon-reload"));
        // Owner/acl follow the user; group lines follow the group.
        let split = service_user_script("debian", "devuser", "devgroup").unwrap();
        assert!(!split.contains("User=devuser"));
        assert!(!split.contains("Group=devgroup"));
        assert!(!split.contains("chown -R devuser:devgroup"));
        assert!(split.contains("listen.owner = devuser"));
        assert!(split.contains("listen.group = devgroup"));
        assert!(split.find("daemon-reload").unwrap() < split.find("systemctl restart").unwrap());
        assert!(service_user_script("arch", TEST_USER, TEST_USER).is_none());
    }

    #[test]
    fn service_user_script_applies_session_and_memory_fixes() {
        // Session dir stays user-owned (mkdir runs as root, so chown back).
        let deb = service_user_script("debian", TEST_USER, TEST_USER).unwrap();
        let session = format!("{}/WebRoots/.php-session", home_dir_for(TEST_USER));
        assert!(deb.contains(&format!("mkdir -p {}", session)));
        assert!(deb.contains(&format!("chown {}:{} ", TEST_USER, TEST_USER)));
        assert!(!deb.contains("chown -R"));
        assert!(deb.contains(&format!("chmod 700 {}", session)));
        assert!(!deb.contains("sudo"));
        // Pool insertions carry an append fallback for commented/absent stock lines.
        assert!(deb.contains(&format!("php_value[session.save_path] = {}", session)));
        assert!(deb.contains("php_admin_value[memory_limit] = 512M"));
        assert!(deb.contains("/etc/php/8.4/cli/php.ini"));
        assert!(deb.contains("s/^memory_limit.*/memory_limit = 512M/"));
        let fed = service_user_script("fedora", "devuser", "devgroup").unwrap();
        let fsession = format!("{}/WebRoots/.php-session", home_dir_for("devuser"));
        assert!(fed.contains(&format!("mkdir -p {}", fsession)));
        assert!(fed.contains("chown devuser:devgroup"));
        assert!(fed.contains(&format!("php_value[session.save_path] = {}", fsession)));
        assert!(fed.contains("php_admin_value[memory_limit] = 512M"));
        assert!(fed.contains("/etc/php.ini"));
        // Fixes land before the service restarts.
        for script in [&deb, &fed] {
            let fixes = script.find("mkdir -p").unwrap();
            let ini = script.find("memory_limit = 512M/").unwrap();
            let restart = script.find("systemctl restart").unwrap();
            assert!(fixes < ini);
            assert!(ini < restart);
        }
        // Full install scripts stay valid shells.
        for (v, o) in [("debian", "install"), ("fedora", "install")] {
            let argv = package_command(v, o, TEST_USER).unwrap();
            let body = argv.last().unwrap();
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", body])
                .status()
                .expect("sh must exist for syntax check");
            assert!(status.success(), "shell syntax invalid for {}/{}: {}", v, o, body);
        }
    }

    #[test]
    fn sudo_group_parsing() {
        // Only an exact `sudo` token counts.
        assert!(groups_has_sudo(&format!("{} : {} sudo docker", TEST_USER, TEST_USER)));
        assert!(groups_has_sudo(&format!("{} : {} sudo: docker", TEST_USER, TEST_USER)));
        assert!(groups_has_sudo("sudo"));
        assert!(!groups_has_sudo(&format!("{} : {} docker", TEST_USER, TEST_USER)));
        assert!(!groups_has_sudo(&format!("{} : {} sudoers admins", TEST_USER, TEST_USER)));
        assert!(!groups_has_sudo(""));
    }

    #[test]
    fn prereq_probes_on_this_host() {
        // Host fixture: certutil, pkexec, sudo membership.
        assert!(certutil_present());
        assert!(pkexec_installed());
        assert!(in_sudo_group());
        assert!(whereis_has_binary(
            "certutil: /usr/bin/certutil /usr/share/man/man1/certutil.1.gz",
            "certutil"
        ));
        assert!(!whereis_has_binary("certutil:", "certutil"));
        // Stale whereis entries stay rejected; the path must exist on disk.
        assert!(!whereis_has_binary(
            "certutil: /nonexistent/certutil",
            "certutil"
        ));
        let (ok, in_sudo, needs_sudo, pkgs, variant, selinux_off) = prereq_probe();
        assert!(ok);
        assert!(in_sudo);
        assert!(!needs_sudo);
        assert_eq!(pkgs, "pkexec libnss3-tools");
        assert_eq!(variant, "Debian");
        assert_eq!(selinux_off, selinux_disabled_in_config());
    }

    #[test]
    fn selinux_config_parsing() {
        // Last effective SELINUX= wins; SELINUXTYPE= never matches.
        assert!(parse_selinux_config_disabled("SELINUX=disabled\nSELINUXTYPE=targeted\n"));
        assert!(parse_selinux_config_disabled("# comment\n  SELINUX=Disabled  # inline\n"));
        assert!(!parse_selinux_config_disabled("SELINUX=enforcing\nSELINUXTYPE=targeted\n"));
        assert!(!parse_selinux_config_disabled("SELINUX=permissive\n"));
        assert!(!parse_selinux_config_disabled("#SELINUX=disabled\n"));
        assert!(!parse_selinux_config_disabled("SELINUXTYPE=targeted\n"));
        assert!(!parse_selinux_config_disabled(""));
        assert!(parse_selinux_config_disabled("SELINUX=enforcing\nSELINUX=disabled\n"));
        assert!(!parse_selinux_config_disabled("SELINUX=disabled\nSELINUX=enforcing\n"));
    }

    #[test]
    fn prereq_gate_rule() {
        // Args: (certutil, pkexec, fedora_like, selinux_off).
        assert!(prereq_satisfied(true, true, false, false));
        assert!(!prereq_satisfied(true, false, false, false));
        assert!(!prereq_satisfied(false, true, false, false));
        assert!(!prereq_satisfied(false, false, false, false));
        // Fedora gates SELinux, not pkexec.
        assert!(prereq_satisfied(true, true, true, true));
        assert!(prereq_satisfied(true, false, true, true));
        assert!(!prereq_satisfied(true, true, true, false));
        assert!(!prereq_satisfied(false, true, true, true));
    }

    #[test]
    fn prereq_install_scripts_render_per_distro() {
        // Debian runs interactive sudo in the terminal and holds the window open.
        let deb = prereq_install_script("debian", DEBIAN_PREREQ_PKGS, false).unwrap();
        assert!(deb.contains("apt-get update && apt-get install -y"));
        assert!(deb.contains("pkexec"));
        assert!(deb.contains("libnss3-tools"));
        assert!(deb.contains("DEBIAN_FRONTEND=noninteractive"));
        assert!(deb.starts_with("sudo sh -c '"));
        assert!(deb.contains("exec sh"));
        assert!(!deb.contains("-n"));
        // Body uses double quotes only inside `sudo sh -c '...'`.
        let fed = prereq_install_script("fedora", FEDORA_PREREQ_PKGS, false).unwrap();
        assert!(fed.contains("dnf install -y nss-tools"));
        assert!(fed.contains("sed -i \"s/^SELINUX=.*/SELINUX=disabled/\" /etc/selinux/config"));
        assert!(fed.contains("setenforce 0"));
        assert!(fed.starts_with("sudo sh -c '"));
        assert!(fed.contains("exec sh"));
        assert!(prereq_install_script("arch", FEDORA_PREREQ_PKGS, false).is_none());
        // Reboot variant: reboot stays inside sudo with no `exec sh` hold; skips reboot when already disabled.
        let re = prereq_install_script("fedora", FEDORA_PREREQ_PKGS, true).unwrap();
        assert!(re.contains("dnf install -y nss-tools"));
        assert!(re.contains("SELINUX=disabled"));
        assert!(re.contains("&& reboot"));
        assert!(re.contains("grep -Eqi"));
        assert!(re.contains("already disabled"));
        assert!(!re.contains("exec sh"));
        assert_eq!(re.matches('\'').count(), 2);
        // `sh -n` parses without executing.
        for body in [&deb, &fed, &re] {
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", body])
                .status()
                .expect("sh must exist for syntax check");
            assert!(status.success(), "shell syntax invalid: {}", body);
        }
    }

    #[test]
    fn fixsudo_script_stages_adduser_then_reboot() {
        // Enrollment stays idempotent; reboot stays chained so a failed adduser never reboots.
        let s = fixsudo_script_content(TEST_USER);
        assert!(s.starts_with("#!/bin/sh"));
        assert!(s.contains(&format!("CURRENT_USER={}", TEST_USER)));
        assert!(s.contains("id -nG"));
        assert!(s.contains("grep -qx sudo"));
        assert!(s.contains("already in the sudo group"));
        assert!(s.contains("exit 0"));
        assert!(s.contains("/usr/bin/su - root -c \"/usr/sbin/adduser $CURRENT_USER sudo && reboot\""));
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", &s])
            .status()
            .expect("sh must exist for syntax check");
        assert!(status.success(), "shell syntax invalid: {}", s);
    }

    #[test]
    fn terminal_detection_runs_without_prompt() {
        // Liveness only; never prompts.
        let _ = terminal_emulator();
    }

    #[test]
    fn pkcheck_probe_runs_without_prompt() {        // Must never prompt or panic; value depends on host auth state
        // (this Debian dev host has the sudo-group rule -> true expected,
        // but assert only liveness, headless CI may lack an active session).
        let _ = pkexec_functional_active();
    }

    #[test]
    fn rules_probe_reads_marker_content() {
        let dir = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("ducknet-test-polkit-{}", stamp));
        let ps = path.to_string_lossy().to_string();
        // Absent file -> not active.
        assert!(!rules_file_active(&format!("{}-missing", ps), "sudo"));
        std::fs::write(&path, polkit_rule_content("sudo")).unwrap();
        assert!(rules_file_active(&ps, "sudo"));
        assert!(!rules_file_active(&ps, "wheel"));
        // Foreign content (no YES marker) -> not active.
        std::fs::write(&path, "// admin rule\n").unwrap();
        assert!(!rules_file_active(&ps, "sudo"));
        let _ = std::fs::remove_file(&path);
    }

    /// Minimal xbel fixture.
    fn xbel_fixture() -> String {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel>\n <bookmark href=\"file:///home/testdev/Desktop\">\n  <title>Desktop</title>\n </bookmark>\n <bookmark href=\"file:///home/testdev/Documents\">\n  <title>Documents</title>\n </bookmark>\n</xbel>\n"
            .to_string()
    }

    #[test]
    fn dev_places_snippet_renders_all_bookmarks() {
        let s = dev_places_snippet(TEST_USER);
        assert!(s.contains(&format!("file:///home/{}/Coding", TEST_USER)));
        assert!(s.contains(&format!("file:///home/{}/MyApps", TEST_USER)));
        assert!(s.contains(&format!("file:///home/{}/WebRoots", TEST_USER)));
        assert!(s.contains("<title>Coding</title>"));
        assert!(s.contains("<title>MyApps</title>"));
        assert!(s.contains("<title>WebRoots</title>"));
        assert!(s.contains("<bookmark:icon name=\"folder-script\"/>"));
        assert!(s.contains("<bookmark:icon name=\"folder-extension\"/>"));
        assert!(s.contains("<bookmark:icon name=\"folder-html\"/>"));
        assert!(s.contains("<ID>1711111111</ID>"));
        assert!(s.contains("<ID>1711111112</ID>"));
        assert!(s.contains("<ID>1711111113</ID>"));
    }

    #[test]
    fn dev_places_presence_needs_all_hrefs() {
        assert!(!dev_places_present(&xbel_fixture(), TEST_USER));
        // Title alone pointing elsewhere must not count.
        let decoy = xbel_fixture().replace("file:///home/testdev/Documents", "file:///home/testdev/Elsewhere")
            + "<!-- <title>Coding</title><title>MyApps</title><title>WebRoots</title> -->";
        assert!(!dev_places_present(&decoy, TEST_USER));
        // Inserted content counts; other users' hrefs must not.
        let inserted = insert_dev_places(&xbel_fixture(), TEST_USER).unwrap();
        assert!(dev_places_present(&inserted, TEST_USER));
        assert!(!dev_places_present(&inserted, "otherdev"));
        // Only two hrefs present -> still not done.
        let half = xbel_fixture().replace("</xbel>", &format!("<bookmark href=\"file:///home/{}/Coding\"></bookmark>\n<bookmark href=\"file:///home/{}/MyApps\"></bookmark>\n</xbel>", TEST_USER, TEST_USER));
        assert!(!dev_places_present(&half, TEST_USER));
    }

    #[test]
    fn dev_places_insert_lands_after_desktop() {
        let fixture = xbel_fixture();
        let out = insert_dev_places(&fixture, TEST_USER).unwrap();
        // All three bookmarks inserted exactly once.
        assert_eq!(out.matches(&format!("file:///home/{}/Coding", TEST_USER)).count(), 1);
        assert_eq!(out.matches(&format!("file:///home/{}/MyApps", TEST_USER)).count(), 1);
        assert_eq!(out.matches(&format!("file:///home/{}/WebRoots", TEST_USER)).count(), 1);
        // Ordering: Desktop close -> Coding -> MyApps -> WebRoots -> Documents.
        let desktop_close = out.find("</bookmark>").unwrap();
        let coding = out.find(&format!("file:///home/{}/Coding", TEST_USER)).unwrap();
        let myapps = out.find(&format!("file:///home/{}/MyApps", TEST_USER)).unwrap();
        let webroots = out.find(&format!("file:///home/{}/WebRoots", TEST_USER)).unwrap();
        let docs = out.find("file:///home/testdev/Documents").unwrap();
        assert!(desktop_close < coding);
        assert!(coding < myapps);
        assert!(myapps < webroots);
        assert!(webroots < docs);
        // Second insert would duplicate, so callers check presence first.
        assert!(dev_places_present(&out, TEST_USER));
        // Missing Desktop marker -> None (never corrupt the file).
        assert!(insert_dev_places("<xbel></xbel>", TEST_USER).is_none());
        assert!(insert_dev_places("", TEST_USER).is_none());
    }

    #[test]
    fn ensure_dev_folders_creates_dirs_and_bookmarks_once() {
        let root = std::env::temp_dir().join(format!(
            "ducknet-test-devhome-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.to_string_lossy().to_string();
        std::fs::create_dir_all(format!("{}/.local/share", home)).unwrap();
        std::fs::write(
            format!("{}/.local/share/user-places.xbel", home),
            xbel_fixture(),
        )
        .unwrap();
        // First run creates all three dirs and inserts: changed.
        assert_eq!(ensure_dev_folders(TEST_USER, &home), Ok(true));
        for dir in ["Coding", "MyApps", "WebRoots"] {
            assert!(std::path::Path::new(&format!("{}/{}", home, dir)).is_dir());
        }
        let content =
            std::fs::read_to_string(format!("{}/.local/share/user-places.xbel", home)).unwrap();
        assert!(dev_places_present(&content, TEST_USER));
        // Second run: everything present, no change.
        assert_eq!(ensure_dev_folders(TEST_USER, &home), Ok(false));
        // Missing Places file errors (non-Plasma session).
        assert!(ensure_dev_folders(TEST_USER, "/nonexistent-ducknet-home").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dev_places_paths_derive_from_home() {
        assert_eq!(
            user_places_path("/home/testdev"),
            "/home/testdev/.local/share/user-places.xbel"
        );
        assert_eq!(
            user_places_path("/home/testdev/"),
            "/home/testdev/.local/share/user-places.xbel"
        );
        // Missing file probes false (never prompt, never panic).
        assert!(!dev_places_added_probe(
            TEST_USER,
            "/nonexistent-ducknet-home"
        ));
    }
}
