// SPDX-License-Identifier: MIT
//! Database manager: install PostgreSQL or MariaDB for WebApp development.
//! Probes with unprivileged systemctl reads; installs run as background ops with live log.

use cxx_qt_lib::{QString, QStringList};
use std::pin::Pin;
use std::process::{Child, Command};
use std::sync::Mutex;

use crate::common::{
    distro_id, distro_is_debian_like, distro_is_fedora_like, family, family_str, home_dir,
    log_debug, op_log_tail, privileged_spawn, to_qstringlist, valid_password, NEEDS_ROOT,
};

/// Distro stock package sets; no third-party repos needed.
const DEBIAN_POSTGRES_PKGS: &[&str] = &["postgresql", "postgresql-client", "postgresql-contrib"];
const DEBIAN_MARIADB_PKGS: &[&str] = &["mariadb-server", "mariadb-client"];
const FEDORA_POSTGRES_PKGS: &[&str] = &["postgresql-server", "postgresql-contrib"];
const FEDORA_MARIADB_PKGS: &[&str] = &["mariadb", "mariadb-server"];

/// systemd units on both families.
const POSTGRES_UNIT: &str = "postgresql.service";
const MARIADB_UNIT: &str = "mariadb.service";

/// pg_hba.conf paths for the scram-sha-256 fix.
/// Fedora path exists only after initdb, so sed runs after it.
const DEBIAN_PG_HBA: &str = "/etc/postgresql/17/main/pg_hba.conf";
const FEDORA_PG_HBA: &str = "/var/lib/pgsql/data/pg_hba.conf";

/// System databases never shown in Current Databases (noise on fresh installs).
const MYSQL_SYSTEM_DBS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

const DB_OP_LOG: &str = "/tmp/ducknet-db-op.log";

/// Last-known database lists (~/.cache/ducknet-dev-tool/db-lists).
/// The page paints from cache on open (silent — no privilege prompt) and
/// overwrites it on every live refresh/create/delete. DUCKNET_DB_CACHE
/// overrides the path for tests (same idiom as DUCKNET_OS_RELEASE).
fn db_cache_path() -> String {
    if let Ok(p) = std::env::var("DUCKNET_DB_CACHE") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    format!("{}/.cache/ducknet-dev-tool/db-lists", home_dir())
}

/// Persist both lists; best-effort (a missing cache just means an empty
/// first paint, never an error).
fn write_db_cache(postgres: &[String], mariadb: &[String]) {
    if let Some(parent) = std::path::Path::new(&db_cache_path()).parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let mut text = String::from("[postgres]\n");
    for d in postgres {
        text.push_str(d);
        text.push('\n');
    }
    text.push_str("[mariadb]\n");
    for d in mariadb {
        text.push_str(d);
        text.push('\n');
    }
    let _ = std::fs::write(db_cache_path(), text);
}

/// Read the cache back; entries re-validated (the file is user-writable).
/// Missing/unreadable file yields empty lists.
fn read_db_cache() -> (Vec<String>, Vec<String>) {
    let mut pg = Vec::new();
    let mut my = Vec::new();
    let text = std::fs::read_to_string(db_cache_path()).unwrap_or_default();
    let mut section = "";
    for line in text.lines() {
        let t = line.trim();
        if t == "[postgres]" {
            section = "pg";
            continue;
        }
        if t == "[mariadb]" {
            section = "my";
            continue;
        }
        if !valid_db_ident(t) {
            continue;
        }
        let list = if section == "pg" {
            &mut pg
        } else if section == "my" {
            &mut my
        } else {
            continue;
        };
        if !list.iter().any(|d| d == t) {
            list.push(t.to_string());
        }
    }
    (pg, my)
}

/// Background install op; package installs run for minutes, never block the UI.
struct DbOp {
    child: Child,
    title: String,
    done: bool,
    ok: bool,
    code: Option<i32>,
}

static DB_OP: Mutex<Option<DbOp>> = Mutex::new(None);

/// Parse `systemctl status` output; exists counts as installed even when stopped.
/// Detail wording matches nginxman for consistent status labels.
fn parse_db_status(output: &str) -> (bool, bool, String) {
    if output.contains("could not be found")
        || output.contains("not loaded")
        || output.contains("No such file")
    {
        return (false, false, "not installed".to_string());
    }
    let running = output.contains("active (running)");
    let detail = if running {
        "active (running)".to_string()
    } else if output.contains("inactive (dead)") {
        "installed — inactive (dead)".to_string()
    } else if output.contains("failed") {
        "installed — service failed".to_string()
    } else {
        "installed — not running".to_string()
    };
    (true, running, detail)
}

/// Probe one unit; unprivileged read, never prompts.
fn probe_db(unit: &str) -> (bool, bool, String) {
    match Command::new("systemctl").args(["status", unit]).output() {
        Err(e) => (false, false, format!("Failed to run systemctl status: {}", e)),
        Ok(out) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            parse_db_status(&text)
        }
    }
}

/// Parse `systemctl is-enabled` output; true only for boot-starting states.
fn parse_systemd_enabled(output: &str) -> bool {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .any(|l| l == "enabled" || l == "enabled-runtime")
}

/// Probe whether the MariaDB unit starts on boot.
fn probe_mariadb_enabled() -> bool {
    match Command::new("systemctl")
        .args(["is-enabled", MARIADB_UNIT])
        .output()
    {
        Err(_) => false,
        Ok(out) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            parse_systemd_enabled(&text)
        }
    }
}

/// Enable-button rule: Fedora-only, shown when installed but not enabled.
/// Fedora KDE pre-installs mariadb-server disabled, so adoption needs one click.
fn mariadb_enable_visible(fedora_like: bool, installed: bool, enabled: bool) -> bool {
    fedora_like && installed && !enabled
}

/// Parse `list-units` rows; any SUB running counts, umbrella `exited` does not.
/// Detail names the running instance; installed means any LOAD loaded.
fn parse_postgres_units(output: &str) -> (bool, bool, String) {
    let mut units: Vec<(String, String, String)> = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        if !cols[0].starts_with("postgresql") {
            continue;
        }
        units.push((cols[0].to_string(), cols[1].to_string(), cols[3].to_string()));
    }
    if units.is_empty() {
        return (false, false, "not installed".to_string());
    }
    units.sort();
    let installed = units.iter().any(|(_, load, _)| load == "loaded");
    if !installed {
        return (false, false, "not installed".to_string());
    }
    match units.iter().find(|(_, _, sub)| sub == "running") {
        Some((unit, _, _)) => (true, true, format!("{} — active (running)", unit)),
        None => (true, false, "installed — inactive (dead)".to_string()),
    }
}

/// Parse `list-unit-files` rows; true when any postgresql unit file exists.
/// Catches never-started installs absent from `list-units`.
fn parse_postgres_unit_files(output: &str) -> bool {
    output.lines().any(|l| {
        let name = l.split_whitespace().next().unwrap_or("");
        name.starts_with("postgresql") && name != "UNIT"
    })
}

/// Probe PostgreSQL across both layouts; falls back to unit files when never started.
fn probe_postgres() -> (bool, bool, String) {
    match Command::new("systemctl")
        .args(["list-units", "--all", "--no-legend", "--plain", "postgresql*"])
        .output()
    {
        Err(e) => (false, false, format!("Failed to run systemctl status: {}", e)),
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let (installed, running, detail) = parse_postgres_units(&text);
            if installed {
                return (installed, running, detail);
            }
            match Command::new("systemctl")
                .args(["list-unit-files", "--no-legend", "postgresql*"])
                .output()
            {
                Err(e) => (false, false, format!("Failed to run systemctl status: {}", e)),
                Ok(files) => {
                    if parse_postgres_unit_files(&String::from_utf8_lossy(&files.stdout)) {
                        (true, false, "installed — inactive (dead)".to_string())
                    } else {
                        (false, false, "not installed".to_string())
                    }
                }
            }
        }
    }
}

/// Build privileged install argv from hardcoded package sets.
/// Reload follows enable --now so scram-sha-256 bites after auto-start; `postgres` peer line must survive for runuser psql.
/// Fedora initdb is best-effort; scripts run as root, so no sudo prefix.
fn db_command(variant: &str, db: &str) -> Option<Vec<String>> {
    let script = match (variant, db) {
        ("debian", "postgres") => format!(
            "export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y {} && sed -i.bak -E -e 's/^(local\\s+all\\s+all\\s+)peer/\\1scram-sha-256/' {} && systemctl enable --now {} && systemctl reload {}",
            DEBIAN_POSTGRES_PKGS.join(" "),
            DEBIAN_PG_HBA,
            POSTGRES_UNIT,
            POSTGRES_UNIT
        ),
        ("debian", "mariadb") => format!(
            "export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y {} && systemctl enable --now {}",
            DEBIAN_MARIADB_PKGS.join(" "),
            MARIADB_UNIT
        ),
        ("fedora", "postgres") => format!(
            "dnf install -y {} && {{ postgresql-setup --initdb || echo \"NOTE: initdb skipped (cluster already initialized).\"; }} && sed -i.bak -E -e 's/^(local\\s+all\\s+all\\s+)peer/\\1scram-sha-256/' -e 's/^(host\\s+all\\s+all\\s+(127\\.0\\.0\\.1\\/32|::1\\/128)\\s+)ident/\\1scram-sha-256/' {} && grep -qE '^local\\s+all\\s+postgres\\s+peer' {} || sed -i -E '/^local\\s+all\\s+all\\s+/i local all postgres peer' {} && systemctl enable --now {} && systemctl reload {}",
            FEDORA_POSTGRES_PKGS.join(" "),
            FEDORA_PG_HBA,
            FEDORA_PG_HBA,
            FEDORA_PG_HBA,
            POSTGRES_UNIT,
            POSTGRES_UNIT
        ),
        ("fedora", "mariadb") => format!(
            "dnf install -y {} && systemctl enable --now {}",
            FEDORA_MARIADB_PKGS.join(" "),
            MARIADB_UNIT
        ),
        _ => return None,
    };
    Some(vec!["sh".to_string(), "-c".to_string(), script])
}

/// Parse `psql -tA` / `mariadb -N` output into database names.
fn parse_db_list_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Drop MySQL/MariaDB system databases (fresh installs list only noise).
fn filter_mysql_system_dbs(dbs: Vec<String>) -> Vec<String> {
    dbs.into_iter()
        .filter(|d| !MYSQL_SYSTEM_DBS.contains(&d.as_str()))
        .collect()
}

/// Identifier guard for database/user names; quoted in SQL after validation.
/// Rejects hostile input that could break out of the statement.
fn valid_db_ident(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// SQL single-quote literal; keeps the SQL string well-formed.
/// Passwords travel as argv, never through a shell.
fn sql_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// CREATE statements for dev database + owner user.
/// UTF8 via template0: the default template1 may carry another encoding,
/// which would reject the ENCODING clause.
/// CREATE DATABASE needs its own psql invocation; it refuses implicit transaction blocks.
fn postgres_create_statements(db: &str, user: &str, password: &str) -> [String; 2] {
    [
        format!("CREATE USER \"{user}\" WITH PASSWORD {};", sql_squote(password)),
        format!("CREATE DATABASE \"{db}\" OWNER \"{user}\" ENCODING 'UTF8' TEMPLATE template0;"),
    ]
}

/// MariaDB create: UTF-8 by default, utf8mb4 when ticked (4-byte
/// characters/emoji). Charset pair is a hardcoded constant — no user input.
fn mariadb_create_sql(db: &str, user: &str, password: &str, utf8mb4: bool) -> String {
    let (charset, collate) = if utf8mb4 {
        ("utf8mb4", "utf8mb4_unicode_ci")
    } else {
        ("utf8", "utf8_general_ci")
    };
    format!(
        "CREATE DATABASE IF NOT EXISTS `{db}` CHARACTER SET {charset} COLLATE {collate}; CREATE USER IF NOT EXISTS '{user}'@'localhost' IDENTIFIED BY {pw}; GRANT ALL PRIVILEGES ON `{db}`.* TO '{user}'@'localhost'; FLUSH PRIVILEGES;",
        pw = sql_squote(password)
    )
}

/// DROP statements (database only — the owner user is left in place).
/// Names are valid_db_ident-checked by callers, then quoted.
fn postgres_drop_sql(db: &str) -> String {
    format!("DROP DATABASE IF EXISTS \"{db}\";")
}

fn mariadb_drop_sql(db: &str) -> String {
    format!("DROP DATABASE IF EXISTS `{db}`;")
}

/// First (user, host) holding grants on a MariaDB database, from the
/// mysql.db grant table. Host is narrowly validated (hostname/IPv4/%),
/// user goes through valid_db_ident at the call site.
fn parse_grant_holder(output: &str) -> Option<(String, String)> {
    let line = output.lines().map(|l| l.trim()).find(|l| !l.is_empty())?;
    let mut parts = line.split_whitespace();
    let user = parts.next()?.to_string();
    let host = parts.next()?.to_string();
    if host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '%' || c == '_' || c == '-')
        && !host.is_empty()
    {
        Some((user, host))
    } else {
        None
    }
}

/// Per-database backup root (~/BACKUPDB/<engine>/<db>/, engine = the
/// validated kind string, so same-named databases on both engines never
/// share a dir). DUCKNET_BACKUPDB overrides the root for tests; kind and
/// db are both validated by callers, so joins cannot escape.
fn backup_root() -> String {
    if let Ok(p) = std::env::var("DUCKNET_BACKUPDB") {
        if !p.trim().is_empty() {
            return p.trim_end_matches('/').to_string();
        }
    }
    format!("{}/BACKUPDB", home_dir())
}

fn backup_db_dir(kind: &str, db: &str) -> String {
    format!("{}/{}/{}", backup_root(), kind, db)
}

/// Timestamped dump name (<db>-<date>.sql); lexical order is newest-last.
fn backup_filename(db: &str) -> String {
    let stamp = chrono::Local::now().format("%F-%H%M%S").to_string();
    format!("{}-{}.sql", db, stamp)
}

/// Strict backup-file gate: no separators, `<db>-*.sql` shape. The name and
/// dir are both validated, so joins stay inside ~/BACKUPDB/<db>/.
fn valid_backup_file(db: &str, filename: &str) -> bool {
    !filename.is_empty()
        && !filename.contains('/')
        && !filename.contains('\\')
        && !filename.contains("..")
        && filename.ends_with(".sql")
        && filename.starts_with(&format!("{}-", db))
        && filename
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Newest-first dump list for one database (empty when none yet).
fn list_backup_files(kind: &str, db: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(backup_db_dir(kind, db)) {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if e.path().is_file() && valid_backup_file(db, name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out.reverse();
    out
}

/// Non-secret restore metadata written beside each dump run (owner/charset
/// recorded at backup time so restores recreate faithfully).
fn backup_meta_path(kind: &str, db: &str) -> String {
    format!("{}/meta", backup_db_dir(kind, db))
}

fn write_backup_meta(kind: &str, db: &str, owner: &str, charset: &str, collation: &str) {
    let text = format!("owner={}\ncharset={}\ncollation={}\n", owner, charset, collation);
    let _ = std::fs::write(backup_meta_path(kind, db), text);
}

/// Parsed (owner, charset, collation); values distrusted until validated by
/// the caller (the file is user-writable).
fn read_backup_meta(kind: &str, db: &str) -> (String, String, String) {
    let mut owner = String::new();
    let mut charset = String::new();
    let mut collation = String::new();
    let text = std::fs::read_to_string(backup_meta_path(kind, db)).unwrap_or_default();
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("owner=") {
            owner = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("charset=") {
            charset = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("collation=") {
            collation = v.trim().to_string();
        }
    }
    (owner, charset, collation)
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
        #[qproperty(bool, postgres_installed, cxx_name = "postgresInstalled")]
        #[qproperty(QString, postgres_status_text, cxx_name = "postgresStatusText")]
        #[qproperty(bool, postgres_running, cxx_name = "postgresRunning")]
        #[qproperty(bool, mariadb_installed, cxx_name = "mariadbInstalled")]
        #[qproperty(QString, mariadb_status_text, cxx_name = "mariadbStatusText")]
        #[qproperty(bool, mariadb_running, cxx_name = "mariadbRunning")]
        #[qproperty(bool, mariadb_enabled, cxx_name = "mariadbEnabled")]
        #[qproperty(bool, mariadb_enable_visible, cxx_name = "mariadbEnableVisible")]
        #[qproperty(QString, distro_family, cxx_name = "distroFamily")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        #[qproperty(bool, op_active, cxx_name = "opActive")]
        #[qproperty(QString, op_log, cxx_name = "opLog")]
        #[qproperty(bool, op_ok, cxx_name = "opOk")]
        #[qproperty(QString, op_name, cxx_name = "opName")]
        #[qproperty(QStringList, postgres_databases, cxx_name = "postgresDatabases")]
        #[qproperty(QStringList, mariadb_databases, cxx_name = "mariadbDatabases")]
        #[qproperty(QStringList, db_backup_files, cxx_name = "dbBackupFiles")]
        #[qproperty(QString, db_backup_db, cxx_name = "dbBackupDb")]
        type DatabaseManager = super::DatabaseManagerRust;

        /// Re-probe both database units (unprivileged, never prompts).
        #[qinvokable]
        #[cxx_name = "refreshStatus"]
        fn refresh_status(self: Pin<&mut Self>) -> bool;

        /// Start a database install in the background; returns false when busy or unsupported.
        /// The QML dialog polls `pollDbOp`.
        #[qinvokable]
        #[cxx_name = "startDbOp"]
        fn start_db_op(self: Pin<&mut Self>, op: &QString) -> bool;

        /// Poll the background install; refreshes LEDs on completion.
        #[qinvokable]
        #[cxx_name = "pollDbOp"]
        fn poll_db_op(self: Pin<&mut Self>) -> bool;

        /// `systemctl start postgresql.service` (privileged). No-op when running.
        #[qinvokable]
        #[cxx_name = "startPostgres"]
        fn start_postgres(self: Pin<&mut Self>) -> bool;

        /// `systemctl stop postgresql.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "stopPostgres"]
        fn stop_postgres(self: Pin<&mut Self>) -> bool;

        /// `systemctl restart postgresql.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "restartPostgres"]
        fn restart_postgres(self: Pin<&mut Self>) -> bool;

        /// `systemctl start mariadb.service` (privileged). No-op when running.
        #[qinvokable]
        #[cxx_name = "startMariadb"]
        fn start_mariadb(self: Pin<&mut Self>) -> bool;

        /// `systemctl stop mariadb.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "stopMariadb"]
        fn stop_mariadb(self: Pin<&mut Self>) -> bool;

        /// `systemctl restart mariadb.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "restartMariadb"]
        fn restart_mariadb(self: Pin<&mut Self>) -> bool;

        /// Adopt a pre-installed-but-disabled unit with `enable --now`; no-op when already enabled.
        #[qinvokable]
        #[cxx_name = "enableMariadb"]
        fn enable_mariadb(self: Pin<&mut Self>) -> bool;        /// Refresh Current Databases for installed engines only.
        /// Runs privileged queries, so call from explicit actions, never on page open.
        #[qinvokable]
        #[cxx_name = "refreshDbLists"]
        fn refresh_db_lists(self: Pin<&mut Self>) -> bool;

        /// Create a database + owner user in one engine.
        /// utf8mb4 selects the 4-byte charset for MariaDB (PostgreSQL is
        /// always UTF8 and ignores it).
        /// Returns false with a statusMessage when validation or the query fails.
        #[qinvokable]
        #[cxx_name = "createDatabase"]
        fn create_database(
            self: Pin<&mut Self>,
            db: &QString,
            name: &QString,
            user: &QString,
            password: &QString,
            utf8mb4: bool,
        ) -> bool;

        /// Paint last-known lists from the cache file (silent — no prompt).
        #[qinvokable]
        #[cxx_name = "loadCachedDbLists"]
        fn load_cached_db_lists(self: Pin<&mut Self>) -> bool;

        /// Drop one database (owner user left in place).
        /// The QML type-to-confirm gate runs before this is ever called.
        #[qinvokable]
        #[cxx_name = "deleteDatabase"]
        fn delete_database(self: Pin<&mut Self>, db: &QString, name: &QString) -> bool;

        /// Dump one database to ~/BACKUPDB/<db>/<db>-<date>.sql (superuser,
        /// no password needed). Records owner/charset beside it for restores.
        #[qinvokable]
        #[cxx_name = "backupDatabase"]
        fn backup_database(self: Pin<&mut Self>, db: &QString, name: &QString) -> bool;

        /// List dumps for one database, newest first, into dbBackupFiles.
        #[qinvokable]
        #[cxx_name = "loadDbBackups"]
        fn load_db_backups(self: Pin<&mut Self>, db: &QString, name: &QString) -> bool;

        /// Replay one dump back into its database (recreated when missing).
        #[qinvokable]
        #[cxx_name = "restoreDbBackup"]
        fn restore_db_backup(
            self: Pin<&mut Self>,
            db: &QString,
            name: &QString,
            filename: &QString,
        ) -> bool;

        /// Delete one dump file.
        #[qinvokable]
        #[cxx_name = "deleteDbBackup"]
        fn delete_db_backup(
            self: Pin<&mut Self>,
            db: &QString,
            name: &QString,
            filename: &QString,
        ) -> bool;

        /// Set a new password for the database owner (PostgreSQL) or the
        /// grant-holding user (MariaDB). The QML match gate runs first;
        /// this validates length again before applying.
        #[qinvokable]
        #[cxx_name = "changeDbPassword"]
        fn change_db_password(
            self: Pin<&mut Self>,
            db: &QString,
            name: &QString,
            password: &QString,
        ) -> bool;
    }
}

pub struct DatabaseManagerRust {
    postgres_installed: bool,
    postgres_status_text: QString,
    postgres_running: bool,
    mariadb_installed: bool,
    mariadb_status_text: QString,
    mariadb_running: bool,
    mariadb_enabled: bool,
    mariadb_enable_visible: bool,
    distro_family: QString,
    status_message: QString,
    op_active: bool,
    op_log: QString,
    op_ok: bool,
    op_name: QString,
    postgres_databases: QStringList,
    mariadb_databases: QStringList,
    db_backup_files: QStringList,
    db_backup_db: QString,
}

impl Default for DatabaseManagerRust {
    fn default() -> Self {
        let (pg_inst, pg_run, pg_detail) = probe_postgres();
        let (my_inst, my_run, my_detail) = probe_db(MARIADB_UNIT);
        let my_enabled = probe_mariadb_enabled();
        Self {
            postgres_installed: pg_inst,
            postgres_status_text: QString::from(&pg_detail),
            postgres_running: pg_run,
            mariadb_installed: my_inst,
            mariadb_status_text: QString::from(&my_detail),
            mariadb_running: my_run,
            mariadb_enabled: my_enabled,
            mariadb_enable_visible: mariadb_enable_visible(
                distro_is_fedora_like(),
                my_inst,
                my_enabled,
            ),
            distro_family: QString::from(family_str(&family())),
            status_message: QString::default(),
            op_active: false,
            op_log: QString::default(),
            op_ok: false,
            op_name: QString::default(),
            postgres_databases: QStringList::default(),
            mariadb_databases: QStringList::default(),
            db_backup_files: QStringList::default(),
            db_backup_db: QString::default(),
        }
    }
}

impl qobject::DatabaseManager {
    fn apply_probe(mut self: Pin<&mut Self>) {
        let (pg_inst, pg_run, pg_detail) = probe_postgres();
        let (my_inst, my_run, my_detail) = probe_db(MARIADB_UNIT);
        self.as_mut().set_postgres_installed(pg_inst);
        self.as_mut()
            .set_postgres_status_text(QString::from(&pg_detail));
        self.as_mut().set_postgres_running(pg_run);
        self.as_mut().set_mariadb_installed(my_inst);
        self.as_mut()
            .set_mariadb_status_text(QString::from(&my_detail));
        self.as_mut().set_mariadb_running(my_run);
        let my_enabled = probe_mariadb_enabled();
        self.as_mut().set_mariadb_enabled(my_enabled);
        self.as_mut().set_mariadb_enable_visible(mariadb_enable_visible(
            distro_is_fedora_like(),
            my_inst,
            my_enabled,
        ));
        self.as_mut()
            .set_distro_family(QString::from(family_str(&family())));
        // Log the enable gate so a hidden button stays diagnosable.
        log_debug(&format!(
            "[ducknet] db status: family={} postgres_installed={} running={} ({}) mariadb_installed={} running={} enabled={} enable_visible={} ({})",
            family_str(&family()), pg_inst, pg_run, pg_detail, my_inst, my_run, my_enabled, self.mariadb_enable_visible, my_detail
        ));
    }

    fn refresh_status(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().apply_probe();
        true
    }

    /// Helper: run a privileged command, return combined output on success.
    fn priv_step(argv: &[&str]) -> Result<String, String> {
        match crate::common::privileged_output(argv) {
            Ok(out) => {
                let combined = format!(
                    "{} {}",
                    String::from_utf8_lossy(&out.stdout).trim(),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                if out.status.success() {
                    Ok(String::from_utf8_lossy(&out.stdout).to_string())
                } else {
                    Err(combined.trim().to_string())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// List PostgreSQL databases; template and maintenance DBs excluded in SQL.
    /// Drops to postgres via runuser to use peer socket auth.
    fn list_postgres_dbs() -> Result<Vec<String>, String> {
        let out = Self::priv_step(&[
            "runuser", "-u", "postgres", "--",
            "psql", "-tA", "-v", "ON_ERROR_STOP=1", "-c",
            "SELECT datname FROM pg_database WHERE datistemplate = false AND datname <> 'postgres' ORDER BY 1;",
        ])?;
        Ok(parse_db_list_output(&out))
    }

    /// List MariaDB databases; system DBs filtered client-side.
    /// Runs as root via unix_socket auth, so no password needed.
    fn list_mariadb_dbs() -> Result<Vec<String>, String> {
        let out = Self::priv_step(&["mariadb", "-N", "-e", "SHOW DATABASES;"])?;
        Ok(filter_mysql_system_dbs(parse_db_list_output(&out)))
    }

    fn refresh_db_lists(mut self: Pin<&mut Self>) -> bool {
        // Skip missing engines so a refresh never prompts needlessly.
        let (pg_inst, _, _) = probe_postgres();
        let (my_inst, _, _) = probe_db(MARIADB_UNIT);
        let mut ok = true;
        let mut pg_cache = Vec::new();
        let mut my_cache = Vec::new();
        if pg_inst {
            match Self::list_postgres_dbs() {
                Ok(dbs) => {
                    pg_cache = dbs.clone();
                    self.as_mut().set_postgres_databases(to_qstringlist(&dbs))
                }
                Err(e) => {
                    ok = false;
                    let msg = format!("PostgreSQL list failed: {} {NEEDS_ROOT}", e);
                    self.as_mut().set_status_message(QString::from(&msg));
                    log_debug(&format!("[ducknet] {}", msg));
                }
            }
        } else {
            self.as_mut().set_postgres_databases(QStringList::default());
        }
        if my_inst {
            match Self::list_mariadb_dbs() {
                Ok(dbs) => {
                    my_cache = dbs.clone();
                    self.as_mut().set_mariadb_databases(to_qstringlist(&dbs))
                }
                Err(e) => {
                    ok = false;
                    let msg = format!("MariaDB list failed: {} {NEEDS_ROOT}", e);
                    self.as_mut().set_status_message(QString::from(&msg));
                    log_debug(&format!("[ducknet] {}", msg));
                }
            }
        } else {
            self.as_mut().set_mariadb_databases(QStringList::default());
        }
        if ok {
            // Fresh read — persist for the silent page-open paint.
            write_db_cache(&pg_cache, &my_cache);
            log_debug("[ducknet] database lists refreshed");
        }
        ok
    }

    /// Paint last-known lists from the cache file (silent — no prompt).
    /// False when no cache exists yet; lists then stay as they were.
    fn load_cached_db_lists(mut self: Pin<&mut Self>) -> bool {
        let path = db_cache_path();
        if std::path::Path::new(&path).exists() {
            let (pg, my) = read_db_cache();
            self.as_mut().set_postgres_databases(to_qstringlist(&pg));
            self.as_mut().set_mariadb_databases(to_qstringlist(&my));
            log_debug(&format!(
                "[ducknet] database lists loaded from cache ({} postgres, {} mariadb)",
                pg.len(),
                my.len()
            ));
            true
        } else {
            false
        }
    }

    fn create_database(
        mut self: Pin<&mut Self>,
        db: &QString,
        name: &QString,
        user: &QString,
        password: &QString,
        utf8mb4: bool,
    ) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        let user = user.to_string();
        let password = password.to_string();
        if kind != "postgres" && kind != "mariadb" {
            self.as_mut().set_status_message(QString::from("Unknown database type."));
            return false;
        }
        if !valid_db_ident(&name) {
            self.as_mut().set_status_message(QString::from(
                "Invalid database name (ASCII letter/underscore first, then letters, digits, _ or $, 1–63 chars).",
            ));
            return false;
        }
        if !valid_db_ident(&user) {
            self.as_mut().set_status_message(QString::from(
                "Invalid database user (ASCII letter/underscore first, then letters, digits, _ or $, 1–63 chars).",
            ));
            return false;
        }
        if !valid_password(&password) {
            self.as_mut().set_status_message(QString::from("Password must be 1–256 characters."));
            return false;
        }
        // Re-check install; the engine may vanish after the page probed.
        let (installed, _, _) = if kind == "postgres" {
            probe_postgres()
        } else {
            probe_db(MARIADB_UNIT)
        };
        if !installed {
            let msg = format!(
                "{} is not installed — install it above first.",
                if kind == "postgres" { "PostgreSQL" } else { "MariaDB" }
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let (label, res) = if kind == "postgres" {
            let mut res: Result<String, String> = Ok(String::new());
            for sql in postgres_create_statements(&name, &user, &password) {
                res = Self::priv_step(&[
                    "runuser", "-u", "postgres", "--",
                    "psql", "-v", "ON_ERROR_STOP=1", "-c", &sql,
                ]);
                if res.is_err() {
                    break;
                }
            }
            ("PostgreSQL", res)
        } else {
            let sql = mariadb_create_sql(&name, &user, &password, utf8mb4);
            ("MariaDB", Self::priv_step(&["mariadb", "-e", &sql]))
        };
        match res {
            Ok(_) => {
                self.as_mut().refresh_db_lists();
                let msg = format!("Database '{}' created in {} (owner '{}').", name, label, user);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("{} create failed: {} {NEEDS_ROOT}", label, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    fn delete_database(mut self: Pin<&mut Self>, db: &QString, name: &QString) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        if kind != "postgres" && kind != "mariadb" {
            self.as_mut().set_status_message(QString::from("Unknown database type."));
            return false;
        }
        // Quoted into DROP below — hostile input must never reach it.
        if !valid_db_ident(&name) {
            self.as_mut().set_status_message(QString::from("Database delete not accepted."));
            return false;
        }
        let (installed, _, _) = if kind == "postgres" {
            probe_postgres()
        } else {
            probe_db(MARIADB_UNIT)
        };
        if !installed {
            let msg = format!(
                "{} is not installed — install it above first.",
                if kind == "postgres" { "PostgreSQL" } else { "MariaDB" }
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let (label, res) = if kind == "postgres" {
            let sql = postgres_drop_sql(&name);
            (
                "PostgreSQL",
                Self::priv_step(&[
                    "runuser", "-u", "postgres", "--",
                    "psql", "-v", "ON_ERROR_STOP=1", "-c", &sql,
                ]),
            )
        } else {
            let sql = mariadb_drop_sql(&name);
            ("MariaDB", Self::priv_step(&["mariadb", "-e", &sql]))
        };
        match res {
            Ok(_) => {
                self.as_mut().refresh_db_lists();
                let msg = format!("Database '{}' deleted from {}.", name, label);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("{} delete failed: {} {NEEDS_ROOT}", label, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    /// Shared gate for the backup verbs: known engine, safe identifier,
    /// engine installed. Returns the kind label or an error message.
    fn backup_gate(kind: &str, name: &str) -> Result<&'static str, String> {
        if kind != "postgres" && kind != "mariadb" {
            return Err("Unknown database type.".to_string());
        }
        if !valid_db_ident(name) {
            return Err("Invalid database name.".to_string());
        }
        let installed = if kind == "postgres" {
            probe_postgres().0
        } else {
            probe_db(MARIADB_UNIT).0
        };
        if !installed {
            return Err(format!(
                "{} is not installed — install it above first.",
                if kind == "postgres" { "PostgreSQL" } else { "MariaDB" }
            ));
        }
        Ok(if kind == "postgres" { "PostgreSQL" } else { "MariaDB" })
    }

    fn backup_database(mut self: Pin<&mut Self>, db: &QString, name: &QString) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        let label = match Self::backup_gate(&kind, &name) {
            Ok(l) => l,
            Err(msg) => {
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        // Superuser dump — no password: postgres peer socket, MariaDB root socket.
        let (dump, owner, charset, collation) = if kind == "postgres" {
            let owner = Self::priv_step(&[
                "runuser", "-u", "postgres", "--",
                "psql", "-tA", "-v", "ON_ERROR_STOP=1", "-c",
                &format!("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{}';", name),
            ])
            .ok()
            .map(|o| o.trim().to_string())
            .filter(|o| valid_db_ident(o))
            .unwrap_or_else(|| "postgres".to_string());
            let dump = Self::priv_step(&[
                "runuser", "-u", "postgres", "--",
                "pg_dump", "--clean", "--if-exists", &name,
            ]);
            (dump, owner, String::new(), String::new())
        } else {
            let cc = Self::priv_step(&[
                "mariadb", "-N", "-e",
                &format!("SELECT DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = '{}';", name),
            ])
            .unwrap_or_default();
            let mut parts = cc.split_whitespace();
            let charset = parts.next().unwrap_or("utf8").to_string();
            let collation = parts.next().unwrap_or("utf8_general_ci").to_string();
            let dump = Self::priv_step(&[
                "mariadb-dump", "--single-transaction", "--routines", "--events", &name,
            ]);
            (dump, String::new(), charset, collation)
        };
        let dump = match dump {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("{} backup failed: {} {NEEDS_ROOT}", label, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                return false;
            }
        };
        // Written by the app itself, so the dev owns the files (a root
        // redirect would leave them root-owned in the user's BACKUPDB).
        let dir = backup_db_dir(&kind, &name);
        if std::fs::create_dir_all(&dir).is_err() {
            let msg = format!("Could not create {} — check home directory permissions.", dir);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let file = format!("{}/{}", dir, backup_filename(&name));
        if std::fs::write(&file, &dump).is_err() {
            let msg = format!("Could not write {} — check disk space and permissions.", file);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        write_backup_meta(&kind, &name, &owner, &charset, &collation);
        let msg = format!("Database '{}' backed up to {}.", name, file);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn load_db_backups(mut self: Pin<&mut Self>, db: &QString, name: &QString) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        if (kind != "postgres" && kind != "mariadb") || !valid_db_ident(&name) {
            return false;
        }
        self.as_mut().set_db_backup_db(QString::from(&name));
        self.as_mut()
            .set_db_backup_files(to_qstringlist(&list_backup_files(&kind, &name)));
        true
    }

    fn restore_db_backup(
        mut self: Pin<&mut Self>,
        db: &QString,
        name: &QString,
        filename: &QString,
    ) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        let filename = filename.to_string();
        let label = match Self::backup_gate(&kind, &name) {
            Ok(l) => l,
            Err(msg) => {
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
        };
        if !valid_backup_file(&name, &filename) {
            self.as_mut().set_status_message(QString::from("Unknown backup file."));
            return false;
        }
        let path = format!("{}/{}", backup_db_dir(&kind, &name), filename);
        if std::fs::metadata(&path).is_err() {
            self.as_mut().set_status_message(QString::from("Backup file not found."));
            return false;
        }
        // Owner/charset recorded at backup time; distrusted until validated.
        let (owner, charset, collation) = read_backup_meta(&kind, &name);
        let res = if kind == "postgres" {
            let owner = if valid_db_ident(&owner) {
                // Original login gone (user dropped since)? Fall back to postgres.
                let exists = Self::priv_step(&[
                    "runuser", "-u", "postgres", "--",
                    "psql", "-tA", "-v", "ON_ERROR_STOP=1", "-c",
                    &format!("SELECT 1 FROM pg_roles WHERE rolname = '{}';", owner),
                ])
                .map(|o| o.trim() == "1")
                .unwrap_or(false);
                if exists { owner } else { "postgres".to_string() }
            } else {
                "postgres".to_string()
            };
            let rebuild = format!(
                "DROP DATABASE IF EXISTS \"{name}\"; CREATE DATABASE \"{name}\" OWNER \"{owner}\" ENCODING 'UTF8' TEMPLATE template0;"
            );
            // psql -f runs as the postgres OS user, which cannot read into
            // a 0700 home — stage under /tmp (readable) and clean up after.
            let staged = format!("/tmp/ducknet-restore-{}.sql", name);
            let staged_ok = std::fs::read(&path)
                .map(|bytes| {
                    std::fs::write(&staged, &bytes).is_ok()
                        && Command::new("chmod").arg("644").arg(&staged).output().map(|o| o.status.success()).unwrap_or(false)
                })
                .unwrap_or(false);
            if !staged_ok {
                let msg = format!("Could not stage {} for restore.", filename);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            let r = Self::priv_step(&[
                "runuser", "-u", "postgres", "--",
                "psql", "-v", "ON_ERROR_STOP=1", "-c", &rebuild,
            ])
            .and_then(|_| {
                Self::priv_step(&[
                    "runuser", "-u", "postgres", "--",
                    "psql", "-v", "ON_ERROR_STOP=1", "-d", &name, "-f", &staged,
                ])
            });
            let _ = std::fs::remove_file(&staged);
            r.map(|_| String::new())
        } else {
            let charset = if charset == "utf8" || charset == "utf8mb4" {
                charset
            } else {
                "utf8".to_string()
            };
            let collation = if valid_db_ident(&collation) {
                collation
            } else {
                "utf8_general_ci".to_string()
            };
            let rebuild = format!(
                "DROP DATABASE IF EXISTS `{name}`; CREATE DATABASE `{name}` CHARACTER SET {charset} COLLATE {collation};"
            );
            // Root reads anywhere, so no staging: `source` avoids a shell redirect.
            Self::priv_step(&["mariadb", "-e", &rebuild]).and_then(|_| {
                Self::priv_step(&["mariadb", &name, "-e", &format!("source {}", path)])
            }).map(|_| String::new())
        };
        match res {
            Ok(_) => {
                self.as_mut().refresh_db_lists();
                let msg = format!("Database '{}' restored from {}.", name, filename);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("{} restore failed: {} {NEEDS_ROOT}", label, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    fn delete_db_backup(
        mut self: Pin<&mut Self>,
        db: &QString,
        name: &QString,
        filename: &QString,
    ) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        let filename = filename.to_string();
        if (kind != "postgres" && kind != "mariadb")
            || !valid_db_ident(&name)
            || !valid_backup_file(&name, &filename)
        {
            self.as_mut().set_status_message(QString::from("Unknown backup file."));
            return false;
        }
        let path = format!("{}/{}", backup_db_dir(&kind, &name), filename);
        match std::fs::remove_file(&path) {
            Ok(_) => {
                let msg = format!("Backup {} deleted.", filename);
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().set_db_backup_db(QString::from(&name));
                self.as_mut()
                    .set_db_backup_files(to_qstringlist(&list_backup_files(&kind, &name)));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("Could not delete {}: {}", filename, e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn change_db_password(
        mut self: Pin<&mut Self>,
        db: &QString,
        name: &QString,
        password: &QString,
    ) -> bool {
        let kind = db.to_string();
        let name = name.to_string();
        let password = password.to_string();
        if kind != "postgres" && kind != "mariadb" {
            self.as_mut().set_status_message(QString::from("Unknown database type."));
            return false;
        }
        if !valid_db_ident(&name) {
            self.as_mut().set_status_message(QString::from("Invalid database name."));
            return false;
        }
        if !valid_password(&password) {
            self.as_mut().set_status_message(QString::from("Password must be 1–256 characters."));
            return false;
        }
        let (installed, _, _) = if kind == "postgres" {
            probe_postgres()
        } else {
            probe_db(MARIADB_UNIT)
        };
        if !installed {
            let msg = format!(
                "{} is not installed — install it above first.",
                if kind == "postgres" { "PostgreSQL" } else { "MariaDB" }
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        // Resolve the login whose password changes: the catalog owner on
        // PostgreSQL, the grant-table holder on MariaDB. Both re-validated
        // before interpolation (query output is distrusted).
        let res = if kind == "postgres" {
            let owner = Self::priv_step(&[
                "runuser", "-u", "postgres", "--",
                "psql", "-tA", "-v", "ON_ERROR_STOP=1", "-c",
                &format!("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{}';", name),
            ])
            .ok()
            .map(|o| o.trim().to_string())
            .filter(|o| valid_db_ident(o));
            match owner {
                None => Err("No owner found for this database.".to_string()),
                Some(owner) => {
                    let sql = format!("ALTER USER \"{owner}\" WITH PASSWORD {};", sql_squote(&password));
                    Self::priv_step(&[
                        "runuser", "-u", "postgres", "--",
                        "psql", "-v", "ON_ERROR_STOP=1", "-c", &sql,
                    ])
                }
            }
        } else {
            let holder = Self::priv_step(&[
                "mariadb", "-N", "-e",
                &format!("SELECT User, Host FROM mysql.db WHERE Db = '{}' LIMIT 1;", name),
            ])
            .ok()
            .and_then(|o| parse_grant_holder(&o))
            .filter(|(u, _)| valid_db_ident(u));
            match holder {
                None => Err("No privileged user found for this database.".to_string()),
                Some((user, host)) => {
                    let sql = format!("ALTER USER '{}'@'{}' IDENTIFIED BY {};", user, host, sql_squote(&password));
                    Self::priv_step(&["mariadb", "-e", &sql])
                }
            }
        };
        match res {
            Ok(_) => {
                let msg = format!("Password updated for database '{}'.", name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let label = if kind == "postgres" { "PostgreSQL" } else { "MariaDB" };
                let msg = format!("{} password change failed: {} {NEEDS_ROOT}", label, e);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    /// Run a privileged systemctl action, then re-probe so LEDs stay current.
    fn run_service_action(
        mut self: Pin<&mut Self>,
        service: &str,
        action: &str,
        done_word: &str,
        label: &str,
    ) -> bool {
        let res = crate::common::privileged_output(&["systemctl", action, service]);
        self.as_mut().apply_probe();
        match res {
            Ok(out) if out.status.success() => {
                let msg = format!("{} {}.", label, done_word);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] systemctl {} {}: ok", action, service));
                true
            }
            Ok(out) => {
                let err = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stderr).trim(),
                    String::from_utf8_lossy(&out.stdout).trim()
                );
                let msg = format!(
                    "systemctl {} failed{} {NEEDS_ROOT}",
                    action,
                    if err.is_empty() { String::new() } else { format!(": {}", err) }
                );
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
            Err(e) => {
                let msg = format!("Failed to run systemctl {}: {}", action, e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn start_postgres(mut self: Pin<&mut Self>) -> bool {
        if self.postgres_running {
            self.as_mut()
                .set_status_message(QString::from("PostgreSQL already running."));
            return true;
        }
        self.as_mut()
            .run_service_action(POSTGRES_UNIT, "start", "started", "PostgreSQL")
    }

    fn stop_postgres(mut self: Pin<&mut Self>) -> bool {
        if !self.postgres_running {
            self.as_mut()
                .set_status_message(QString::from("PostgreSQL already stopped."));
            return true;
        }
        self.as_mut()
            .run_service_action(POSTGRES_UNIT, "stop", "stopped", "PostgreSQL")
    }

    fn restart_postgres(mut self: Pin<&mut Self>) -> bool {
        if !self.postgres_running {
            self.as_mut()
                .set_status_message(QString::from("PostgreSQL is stopped — start it first."));
            return false;
        }
        self.as_mut()
            .run_service_action(POSTGRES_UNIT, "restart", "restarted", "PostgreSQL")
    }

    fn start_mariadb(mut self: Pin<&mut Self>) -> bool {
        if self.mariadb_running {
            self.as_mut()
                .set_status_message(QString::from("MariaDB already running."));
            return true;
        }
        self.as_mut()
            .run_service_action(MARIADB_UNIT, "start", "started", "MariaDB")
    }

    fn stop_mariadb(mut self: Pin<&mut Self>) -> bool {
        if !self.mariadb_running {
            self.as_mut()
                .set_status_message(QString::from("MariaDB already stopped."));
            return true;
        }
        self.as_mut()
            .run_service_action(MARIADB_UNIT, "stop", "stopped", "MariaDB")
    }

    fn restart_mariadb(mut self: Pin<&mut Self>) -> bool {
        if !self.mariadb_running {
            self.as_mut()
                .set_status_message(QString::from("MariaDB is stopped — start it first."));
            return false;
        }
        self.as_mut()
            .run_service_action(MARIADB_UNIT, "restart", "restarted", "MariaDB")
    }

    fn enable_mariadb(mut self: Pin<&mut Self>) -> bool {
        // Re-check install; the package may vanish after the page probed.
        if !probe_db(MARIADB_UNIT).0 {
            let msg = "MariaDB is not installed — install it above first.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        if probe_mariadb_enabled() {
            self.as_mut().apply_probe();
            self.as_mut()
                .set_status_message(QString::from("MariaDB already enabled."));
            return true;
        }
        let res = crate::common::privileged_output(&[
            "systemctl",
            "enable",
            "--now",
            MARIADB_UNIT,
        ]);
        self.as_mut().apply_probe();
        match res {
            Ok(out) if out.status.success() => {
                let msg = "MariaDB enabled and started.".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] systemctl enable --now {}: ok", MARIADB_UNIT));
                true
            }
            Ok(out) => {
                let err = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stderr).trim(),
                    String::from_utf8_lossy(&out.stdout).trim()
                );
                let msg = format!(
                    "systemctl enable failed{} {NEEDS_ROOT}",
                    if err.is_empty() { String::new() } else { format!(": {}", err) }
                );
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
            Err(e) => {
                let msg = format!("Failed to run systemctl enable: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn start_db_op(mut self: Pin<&mut Self>, op: &QString) -> bool {
        let op = op.to_string();
        if op != "postgres" && op != "mariadb" {
            self.as_mut()
                .set_status_message(QString::from("Unknown database operation."));
            return false;
        }
        if let Ok(guard) = DB_OP.lock() {
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
                "Database install is only wired for Debian/Fedora-family — this system is '{}'.",
                distro_id()
            );
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        };
        let argv = match db_command(variant, &op) {
            Some(a) => a,
            None => {
                self.as_mut()
                    .set_status_message(QString::from("Unknown database operation."));
                return false;
            }
        };
        let title = match op.as_str() {
            "postgres" => "Install PostgreSQL",
            _ => "Install MariaDB",
        }
        .to_string();
        let header = format!("DuckNet Dev Tool: {} ({})\n$ {}\n\n", title, variant, argv.join(" "));
        if std::fs::write(DB_OP_LOG, &header).is_err() {
            self.as_mut()
                .set_status_message(QString::from("Failed to open database log."));
            return false;
        }
        match privileged_spawn(&argv, DB_OP_LOG) {
            Ok(child) => {
                if let Ok(mut guard) = DB_OP.lock() {
                    *guard = Some(DbOp { child, title: title.clone(), done: false, ok: false, code: None });
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

    fn poll_db_op(mut self: Pin<&mut Self>) -> bool {
        let (active, just_finished, ok, code) = match DB_OP.lock() {
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
        self.as_mut().set_op_log(QString::from(&op_log_tail(DB_OP_LOG)));
        if !active {
            self.as_mut().apply_probe();
        }
        if just_finished {
            self.as_mut().set_op_ok(ok);
            if ok {
                // Best-effort refresh; cached auth rarely prompts twice.
                self.as_mut().refresh_db_lists();
            }
            let title = self.op_name.to_string();
            let title = if title.is_empty() { "Database operation".to_string() } else { title };
            let msg = if ok {
                format!("{} finished successfully.", title)
            } else {
                format!("{} failed{}. See {} for details.",
                    title,
                    code.map(|c| format!(" (exit {})", c)).unwrap_or_default(),
                    DB_OP_LOG)
            };
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] {}", msg));
        }
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_status_parsing() {
        // Running unit: installed + running.
        let (inst, run, detail) = parse_db_status("● postgresql.service active (running) since ...\n   Main PID: 1 (postgres)");
        assert!(inst);
        assert!(run);
        assert!(detail.contains("active (running)"));
        // Stopped unit: installed, not running.
        let (inst, run, detail) = parse_db_status("○ mariadb.service - ...\n     Loaded: loaded\n     Active: inactive (dead)\n");
        assert!(inst);
        assert!(!run);
        assert!(detail.contains("inactive (dead)"));
        // Failed unit: installed, not running.
        let (inst, run, _) = parse_db_status("● mariadb.service - ...\n     Active: failed (Result: exit-code)\n");
        assert!(inst);
        assert!(!run);
        // Missing unit: not installed (both systemctl wordings).
        let (inst, run, detail) = parse_db_status("Unit postgresql.service could not be found.");
        assert!(!inst);
        assert!(!run);
        assert_eq!(detail, "not installed");
        let (inst, _, _) = parse_db_status("Failed to get unit: No such file or directory");
        assert!(!inst);
        // Empty output counts as not-running install.
        let (inst, run, _) = parse_db_status("");
        assert!(inst);
        assert!(!run);
    }

    #[test]
    fn systemd_enabled_parsing() {
        // Boot-enabled states (both start on boot).
        assert!(parse_systemd_enabled("enabled\n"));
        assert!(parse_systemd_enabled("enabled-runtime\n"));
        // Everything else counts as not enabled.
        assert!(!parse_systemd_enabled("disabled\n"));
        assert!(!parse_systemd_enabled("static\n"));
        assert!(!parse_systemd_enabled("masked\n"));
        assert!(!parse_systemd_enabled("indirect\n"));
        assert!(!parse_systemd_enabled("generated\n"));
        assert!(!parse_systemd_enabled(""));
        // Missing unit (not installed): the stderr error shape.
        assert!(!parse_systemd_enabled(
            "Failed to get unit file state for mariadb.service: No such file or directory\n"
        ));
        // Whole-line match only — "disabled" must never read as enabled.
        assert!(!parse_systemd_enabled("disabled"));
    }

    #[test]
    fn enable_button_visibility_rule() {
        // The Fedora pre-install case: installed but not enabled.
        assert!(mariadb_enable_visible(true, true, false));
        // Hidden on non-Fedora even when installed-but-disabled.
        assert!(!mariadb_enable_visible(false, true, false));
        // Hidden when not installed (Install button owns that case).
        assert!(!mariadb_enable_visible(true, false, false));
        assert!(!mariadb_enable_visible(false, false, false));
        // Hidden once enabled (button has done its job).
        assert!(!mariadb_enable_visible(true, true, true));
        assert!(!mariadb_enable_visible(false, true, true));
    }

    #[test]
    fn db_commands_render_per_distro() {
        // Debian: update first, exact sets, enable+start chained.
        let pg = db_command("debian", "postgres").unwrap();
        assert_eq!(&pg[0..2], &["sh".to_string(), "-c".to_string()]);
        assert!(pg[2].contains("apt-get update && apt-get install -y"));
        for p in ["postgresql", "postgresql-client", "postgresql-contrib"] {
            assert!(pg[2].contains(p), "missing {}", p);
        }
        assert!(pg[2].contains("DEBIAN_FRONTEND=noninteractive"));
        assert!(pg[2].contains("systemctl enable --now postgresql.service"));
        // Auth fix keeps postgres peer; reload applies rules after auto-start.
        assert!(pg[2].contains(DEBIAN_PG_HBA));
        assert!(pg[2].contains("scram-sha-256"));
        assert!(!pg[2].contains("sudo"));
        assert!(pg[2].find("apt-get install").unwrap() < pg[2].find("pg_hba.conf").unwrap());
        assert!(pg[2].find("pg_hba.conf").unwrap() < pg[2].find("enable --now").unwrap());
        assert!(pg[2].find("enable --now").unwrap() < pg[2].rfind("systemctl reload").unwrap());
        let my = db_command("debian", "mariadb").unwrap();
        for p in ["mariadb-server", "mariadb-client"] {
            assert!(my[2].contains(p), "missing {}", p);
        }
        assert!(my[2].contains("systemctl enable --now mariadb.service"));
        // Fedora: dnf sets, postgres needs initdb (best-effort), then enable+start.
        let fpg = db_command("fedora", "postgres").unwrap();
        for p in ["postgresql-server", "postgresql-contrib"] {
            assert!(fpg[2].contains(p), "missing {}", p);
        }
        assert!(fpg[2].contains("postgresql-setup --initdb"));
        assert!(fpg[2].contains("cluster already initialized"));
        assert!(fpg[2].contains("systemctl enable --now postgresql.service"));
        // initdb must precede enable so a fresh cluster starts on first boot.
        assert!(fpg[2].find("initdb").unwrap() < fpg[2].find("enable --now").unwrap());
        // Auth fix runs after initdb; reload applies rules.
        assert!(fpg[2].contains(FEDORA_PG_HBA));
        assert!(fpg[2].contains("scram-sha-256"));
        assert!(!fpg[2].contains("sudo"));
        assert!(fpg[2].find("initdb").unwrap() < fpg[2].find("pg_hba.conf").unwrap());
        assert!(fpg[2].find("pg_hba.conf").unwrap() < fpg[2].find("enable --now").unwrap());
        assert!(fpg[2].find("enable --now").unwrap() < fpg[2].rfind("systemctl reload").unwrap());
        // Keeps postgres peer for runuser psql; grep guard keeps it idempotent.
        assert!(fpg[2].contains("local all postgres peer"));
        let fmy = db_command("fedora", "mariadb").unwrap();
        for p in ["mariadb", "mariadb-server"] {
            assert!(fmy[2].contains(p), "missing {}", p);
        }
        assert!(fmy[2].contains("systemctl enable --now mariadb.service"));
        assert!(db_command("arch", "postgres").is_none());
        assert!(db_command("debian", "mysql").is_none());
        // Bodies must parse as valid shells; checked, never executed.
        for argv in [&pg, &my, &fpg, &fmy] {
            let body = argv.join(" ");
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", &body])
                .status()
                .expect("sh must exist for syntax check");
            assert!(status.success(), "shell syntax invalid: {}", body);
        }
    }

    #[test]
    fn db_ident_validation() {
        assert!(valid_db_ident("webapp"));
        assert!(valid_db_ident("_private"));
        assert!(valid_db_ident("app$1"));
        assert!(!valid_db_ident(""));
        assert!(!valid_db_ident("9lives"));
        assert!(!valid_db_ident("my-app"));
        assert!(!valid_db_ident("my app"));
        assert!(!valid_db_ident("db;DROP"));
        assert!(!valid_db_ident("a/b"));
        assert!(!valid_db_ident(&"x".repeat(64)));
        assert!(valid_db_ident(&"x".repeat(63)));
    }

    #[test]
    fn sql_quoting_keeps_statements_wellformed() {
        assert_eq!(sql_squote("s3cret"), "'s3cret'");
        assert_eq!(sql_squote("o'brien"), "'o''brien'");
        // Identifiers are validated upstream; quoting is mechanical.
        let pg = postgres_create_statements("webapp", "webuser", "o'brien");
        assert_eq!(pg[0], "CREATE USER \"webuser\" WITH PASSWORD 'o''brien';");
        assert_eq!(pg[1], "CREATE DATABASE \"webapp\" OWNER \"webuser\" ENCODING 'UTF8' TEMPLATE template0;");
        let my = mariadb_create_sql("webapp", "webuser", "o'brien", false);
        assert!(my.contains("CREATE DATABASE IF NOT EXISTS `webapp` CHARACTER SET utf8 COLLATE utf8_general_ci;"));
        assert!(my.contains("CREATE USER IF NOT EXISTS 'webuser'@'localhost' IDENTIFIED BY 'o''brien';"));
        assert!(my.contains("GRANT ALL PRIVILEGES ON `webapp`.* TO 'webuser'@'localhost';"));
        assert!(my.contains("FLUSH PRIVILEGES;"));
        // Tickbox on: full 4-byte charset, default collation for it.
        let my4 = mariadb_create_sql("webapp", "webuser", "o'brien", true);
        assert!(my4.contains("CREATE DATABASE IF NOT EXISTS `webapp` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"));
        assert!(!my4.contains("utf8_general_ci"));
    }

    #[test]
    fn drop_statements_target_one_database() {
        assert_eq!(postgres_drop_sql("webapp"), "DROP DATABASE IF EXISTS \"webapp\";");
        assert_eq!(mariadb_drop_sql("webapp"), "DROP DATABASE IF EXISTS `webapp`;");
    }

    #[test]
    fn grant_holder_parsing_takes_first_row() {
        assert_eq!(
            parse_grant_holder("webuser\tlocalhost\n"),
            Some(("webuser".to_string(), "localhost".to_string()))
        );
        assert_eq!(
            parse_grant_holder("  webuser   %  \nwebuser localhost\n"),
            Some(("webuser".to_string(), "%".to_string()))
        );
        // Empty output, blank lines and hostile hosts never parse.
        assert_eq!(parse_grant_holder(""), None);
        assert_eq!(parse_grant_holder("\n  \n"), None);
        assert_eq!(parse_grant_holder("webuser\n"), None);
        assert_eq!(parse_grant_holder("webuser a'b\n"), None);
        assert_eq!(parse_grant_holder("webuser a;b\n"), None);
    }

    #[test]
    fn backup_file_gate_keeps_joins_inside_db_dir() {
        assert!(valid_backup_file("webapp", "webapp-2026-09-19-143022.sql"));
        // Wrong db prefix, separators, traversal and suffix games all refuse.
        assert!(!valid_backup_file("webapp", "other-2026-09-19-143022.sql"));
        assert!(!valid_backup_file("webapp", "webapp-2026-09-19-143022.sql.bak"));
        assert!(!valid_backup_file("webapp", "webapp.sql."));
        assert!(!valid_backup_file("webapp", "../webapp-2026.sql"));
        assert!(!valid_backup_file("webapp", "sub/webapp-2026.sql"));
        assert!(!valid_backup_file("webapp", "webapp-2026.sql; rm -rf /"));
        assert!(!valid_backup_file("webapp", ""));
        assert!(!valid_backup_file("webapp", "webapp.sql"));
    }

    #[test]
    fn backup_filenames_sort_newest_last() {
        let f = backup_filename("webapp");
        assert!(f.starts_with("webapp-") && f.ends_with(".sql"));
        assert!(valid_backup_file("webapp", &f));
    }

    #[test]
    fn backup_meta_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "ducknet-test-backupdb-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let rs = root.to_string_lossy().to_string();
        std::env::set_var("DUCKNET_BACKUPDB", &rs);
        std::fs::create_dir_all(backup_db_dir("postgres", "webapp")).unwrap();
        // Newest-first listing over real files.
        std::fs::write(format!("{}/postgres/webapp/webapp-2026-01-01-000000.sql", rs), "old").unwrap();
        std::fs::write(format!("{}/postgres/webapp/webapp-2026-02-01-000000.sql", rs), "new").unwrap();
        std::fs::write(format!("{}/postgres/webapp/notes.txt", rs), "ignored").unwrap();
        assert_eq!(
            list_backup_files("postgres", "webapp"),
            vec![
                "webapp-2026-02-01-000000.sql".to_string(),
                "webapp-2026-01-01-000000.sql".to_string()
            ]
        );
        // Same db name on the other engine is a separate dir.
        assert!(list_backup_files("mariadb", "webapp").is_empty());
        // Meta write + read.
        write_backup_meta("postgres", "webapp", "webuser", "utf8mb4", "utf8mb4_unicode_ci");
        assert_eq!(
            read_backup_meta("postgres", "webapp"),
            (
                "webuser".to_string(),
                "utf8mb4".to_string(),
                "utf8mb4_unicode_ci".to_string()
            )
        );
        // Missing meta reads empty (restore falls back).
        assert_eq!(
            read_backup_meta("postgres", "nosuchdb"),
            (String::new(), String::new(), String::new())
        );
        std::env::remove_var("DUCKNET_BACKUPDB");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn db_cache_round_trips_both_lists() {
        // Isolated path (never the real ~/.cache).
        let path = std::env::temp_dir().join(format!(
            "ducknet-test-dbcache-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ps = path.to_string_lossy().to_string();
        std::env::set_var("DUCKNET_DB_CACHE", &ps);
        // Missing file reads empty.
        assert_eq!(read_db_cache(), (Vec::new(), Vec::new()));
        write_db_cache(&["shop".to_string()], &["blog".to_string(), "wiki".to_string()]);
        assert_eq!(
            read_db_cache(),
            (vec!["shop".to_string()], vec!["blog".to_string(), "wiki".to_string()])
        );
        // Hand-edited junk (bad idents, dupes, stray lines) never surfaces.
        std::fs::write(&path, "[postgres]\nshop\nshop\n../evil\n\n[mariadb]\norphan\n").unwrap();
        assert_eq!(
            read_db_cache(),
            (vec!["shop".to_string()], vec!["orphan".to_string()])
        );
        std::env::remove_var("DUCKNET_DB_CACHE");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn db_list_output_parsing() {
        assert_eq!(
            parse_db_list_output("webapp\nshop\n"),
            vec!["webapp".to_string(), "shop".to_string()]
        );
        // Blank lines and padding never surface.
        assert_eq!(
            parse_db_list_output("  webapp  \n\n  shop\n"),
            vec!["webapp".to_string(), "shop".to_string()]
        );
        assert!(parse_db_list_output("").is_empty());
        // Fresh MariaDB installs list only noise.
        assert!(filter_mysql_system_dbs(parse_db_list_output(
            "information_schema\nmydb\nmysql\nperformance_schema\nsys\n"
        )) == vec!["mydb".to_string()]);
    }

    #[test]
    fn postgres_unit_parsing_covers_instances() {
        // Debian umbrella plus running instance; detail names the instance.
        let deb = "postgresql.service                                                                          loaded active exited    PostgreSQL RDBMS\n\
                   postgresql@17-main.service                                                                  loaded active running   PostgreSQL Cluster 17-main\n";
        let (inst, run, detail) = parse_postgres_units(deb);
        assert!(inst);
        assert!(run);
        assert!(detail.contains("postgresql@17-main.service"));
        assert!(detail.contains("active (running)"));
        // Umbrella exited alone never lights the LED.
        let stopped = "postgresql.service   loaded active exited    PostgreSQL RDBMS\n";
        let (inst, run, detail) = parse_postgres_units(stopped);
        assert!(inst);
        assert!(!run);
        assert!(detail.contains("inactive (dead)"));
        // Fedora: single service, running.
        let fed = "postgresql.service   loaded active running   PostgreSQL database server\n";
        let (inst, run, detail) = parse_postgres_units(fed);
        assert!(inst);
        assert!(run);
        assert!(detail.contains("postgresql.service"));
        // Fedora: installed, stopped.
        let fed_off = "postgresql.service   loaded inactive dead   PostgreSQL database server\n";
        let (inst, run, _) = parse_postgres_units(fed_off);
        assert!(inst);
        assert!(!run);
        // Nothing matching: not installed.
        let (inst, run, detail) = parse_postgres_units("");
        assert!(!inst);
        assert!(!run);
        assert_eq!(detail, "not installed");
        let (inst, _, _) = parse_postgres_units("proc-sys-fs-binfmt_misc.automount   loaded active waiting   -\n");
        assert!(!inst);
    }

    #[test]
    fn postgres_unit_files_cover_never_started() {
        // Template file itself counts (no instances exist yet).
        assert!(parse_postgres_unit_files(
            "postgresql.service                      enabled-runtime\npostgresql@.service                     static\n"
        ));
        assert!(!parse_postgres_unit_files(""));
        // Header line never matches.
        assert!(!parse_postgres_unit_files("UNIT FILE                         STATE\n"));
    }

    #[test]
    fn db_probe_runs_unprivileged_on_this_host() {
        // Never prompts or panics; values depend on host.
        let (_, _, pg_detail) = probe_postgres();
        let (_, _, my_detail) = probe_db(MARIADB_UNIT);
        assert!(!pg_detail.is_empty());
        assert!(!my_detail.is_empty());
        let _ = probe_mariadb_enabled();
    }
}
