use std::env;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("manzil-test-{}-{id}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json_escape(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn manifest(entries: &[(&Path, &Path, bool)]) -> String {
    let files = entries
        .iter()
        .map(|(target, source, clobber)| {
            format!(
                r#"{{"target":"{}","source":"{}","clobber":{}}}"#,
                json_escape(target),
                json_escape(source),
                clobber
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"version":1,"files":[{files}]}}"#)
}

fn write_manifest(path: &Path, entries: &[(&Path, &Path, bool)]) {
    fs::write(path, manifest(entries)).unwrap();
}

fn write_empty_manifest(path: &Path) {
    fs::write(path, r#"{"version":1,"files":[]}"#).unwrap();
}

fn write_source(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn run(home: &Path, new_manifest: &Path, old_manifest: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_manzil"));
    cmd.env("HOME", home).arg(new_manifest);
    if let Some(old_manifest) = old_manifest {
        cmd.arg(old_manifest);
    }
    cmd.output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prune_removes_owned_symlink() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    write_source(&old_source, "old");
    let target = home.join("link");
    symlink(&old_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    write_manifest(&old_json, &[(&target, &old_source, false)]);
    write_empty_manifest(&new_json);

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert!(fs::symlink_metadata(&target).is_err());
}

#[test]
fn prune_preserves_foreign_symlink() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let foreign_source = tmp.path().join("src/foreign");
    write_source(&old_source, "old");
    write_source(&foreign_source, "foreign");
    let target = home.join("link");
    symlink(&foreign_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    write_manifest(&old_json, &[(&target, &old_source, false)]);
    write_empty_manifest(&new_json);

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), foreign_source);
}

#[test]
fn activate_updates_owned_symlink_without_clobber() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let new_source = tmp.path().join("src/new");
    write_source(&old_source, "old");
    write_source(&new_source, "new");
    let target = home.join("link");
    symlink(&old_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    write_manifest(&old_json, &[(&target, &old_source, false)]);
    write_manifest(&new_json, &[(&target, &new_source, false)]);

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), new_source);
}

#[test]
fn activate_preserves_foreign_symlink_without_clobber() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let new_source = tmp.path().join("src/new");
    let foreign_source = tmp.path().join("src/foreign");
    write_source(&old_source, "old");
    write_source(&new_source, "new");
    write_source(&foreign_source, "foreign");
    let target = home.join("link");
    symlink(&foreign_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    write_manifest(&old_json, &[(&target, &old_source, false)]);
    write_manifest(&new_json, &[(&target, &new_source, false)]);

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), foreign_source);
}

#[test]
fn activate_clobbers_foreign_symlink_when_requested() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let new_source = tmp.path().join("src/new");
    let foreign_source = tmp.path().join("src/foreign");
    write_source(&new_source, "new");
    write_source(&foreign_source, "foreign");
    let target = home.join("link");
    symlink(&foreign_source, &target).unwrap();

    let new_json = tmp.path().join("new.json");
    write_manifest(&new_json, &[(&target, &new_source, true)]);

    let output = run(&home, &new_json, None);
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), new_source);
}

#[test]
fn clobber_refuses_directory() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source = tmp.path().join("src/new");
    write_source(&source, "new");
    let target = home.join("dir");
    write_source(&target.join("user-file"), "keep me");

    let new_json = tmp.path().join("new.json");
    write_manifest(&new_json, &[(&target, &source, true)]);

    let output = run(&home, &new_json, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to clobber directory"));
    assert_eq!(
        fs::read_to_string(target.join("user-file")).unwrap(),
        "keep me"
    );
}

#[test]
fn copy_creates_regular_file_with_metadata() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source = tmp.path().join("src/file");
    write_source(&source, "copy me");
    let target = home.join("copied");
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };

    let new_json = tmp.path().join("new.json");
    fs::write(
        &new_json,
        format!(
            r#"{{"version":2,"files":[{{"type":"copy","target":"{}","source":"{}","permissions":"0600","uid":{},"gid":{}}}]}}"#,
            json_escape(&target),
            json_escape(&source),
            uid,
            gid
        ),
    )
    .unwrap();

    let output = run(&home, &new_json, None);
    assert_success(&output);
    assert_eq!(fs::read_to_string(&target).unwrap(), "copy me");
    assert!(!fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn copy_preserves_modified_file_without_clobber() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let new_source = tmp.path().join("src/new");
    write_source(&old_source, "old");
    write_source(&new_source, "new");
    let target = home.join("copied");
    write_source(&target, "user");

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    fs::write(
        &old_json,
        format!(
            r#"{{"version":2,"files":[{{"type":"copy","target":"{}","source":"{}","clobber":false}}]}}"#,
            json_escape(&target),
            json_escape(&old_source)
        ),
    )
    .unwrap();
    fs::write(
        &new_json,
        format!(
            r#"{{"version":2,"files":[{{"type":"copy","target":"{}","source":"{}","clobber":false}}]}}"#,
            json_escape(&target),
            json_escape(&new_source)
        ),
    )
    .unwrap();

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert_eq!(fs::read_to_string(&target).unwrap(), "user");
}

#[test]
fn directory_delete_and_modify_are_supported() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let dir = home.join("dir");
    let doomed = home.join("doomed");
    let modified = home.join("modified");
    write_source(&doomed, "bye");
    write_source(&modified, "mode");

    let new_json = tmp.path().join("new.json");
    fs::write(
        &new_json,
        format!(
            r#"{{"version":2,"files":[
                {{"type":"directory","target":"{}","permissions":"0700"}},
                {{"type":"delete","target":"{}"}},
                {{"type":"modify","target":"{}","permissions":"0600"}}
            ]}}"#,
            json_escape(&dir),
            json_escape(&doomed),
            json_escape(&modified)
        ),
    )
    .unwrap();

    let output = run(&home, &new_json, None);
    assert_success(&output);
    assert!(fs::metadata(&dir).unwrap().is_dir());
    assert!(fs::symlink_metadata(&doomed).is_err());
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&modified).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn missing_new_manifest_version_is_rejected() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source = tmp.path().join("src/file");
    write_source(&source, "source");
    let target = home.join("link");
    let new_json = tmp.path().join("new.json");
    fs::write(
        &new_json,
        format!(
            r#"{{"files":[{{"target":"{}","source":"{}","clobber":false}}]}}"#,
            json_escape(&target),
            json_escape(&source)
        ),
    )
    .unwrap();

    let output = run(&home, &new_json, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing manifest version"));
    assert!(fs::symlink_metadata(&target).is_err());
}

#[test]
fn legacy_old_manifest_without_version_is_accepted() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let new_source = tmp.path().join("src/new");
    write_source(&old_source, "old");
    write_source(&new_source, "new");
    let target = home.join("link");
    symlink(&old_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    fs::write(
        &old_json,
        format!(
            r#"{{"files":[{{"target":"{}","source":"{}","clobber":false}}]}}"#,
            json_escape(&target),
            json_escape(&old_source)
        ),
    )
    .unwrap();
    write_manifest(&new_json, &[(&target, &new_source, false)]);

    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), new_source);
}

#[test]
fn duplicate_targets_in_old_manifest_are_rejected_before_mutation() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let old_source = tmp.path().join("src/old");
    let other_source = tmp.path().join("src/other");
    let new_source = tmp.path().join("src/new");
    write_source(&old_source, "old");
    write_source(&other_source, "other");
    write_source(&new_source, "new");
    let target = home.join("link");
    symlink(&old_source, &target).unwrap();

    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");
    write_manifest(
        &old_json,
        &[
            (&target, &old_source, false),
            (&target, &other_source, false),
        ],
    );
    write_manifest(&new_json, &[(&target, &new_source, false)]);

    let output = run(&home, &new_json, Some(&old_json));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate target"));
    assert_eq!(fs::read_link(&target).unwrap(), old_source);
}

#[test]
fn duplicate_targets_are_rejected_before_mutation() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source_a = tmp.path().join("src/a");
    let source_b = tmp.path().join("src/b");
    write_source(&source_a, "a");
    write_source(&source_b, "b");
    let target = home.join("link");

    let new_json = tmp.path().join("new.json");
    write_manifest(
        &new_json,
        &[(&target, &source_a, false), (&target, &source_b, false)],
    );

    let output = run(&home, &new_json, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate target"));
    assert!(fs::symlink_metadata(&target).is_err());
}

#[test]
fn unknown_manifest_fields_are_rejected() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source = tmp.path().join("src/file");
    write_source(&source, "source");
    let target = home.join("link");

    let new_json = tmp.path().join("new.json");
    fs::write(
        &new_json,
        format!(
            r#"{{"version":1,"files":[{{"target":"{}","source":"{}","clobber":false,"mode":"0644"}}]}}"#,
            json_escape(&target),
            json_escape(&source)
        ),
    )
    .unwrap();

    let output = run(&home, &new_json, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));
    assert!(fs::symlink_metadata(&target).is_err());
}

#[test]
fn parent_components_are_rejected() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source = tmp.path().join("src/file");
    write_source(&source, "source");
    let target = home.join("../escaped");

    let new_json = tmp.path().join("new.json");
    write_manifest(&new_json, &[(&target, &source, false)]);

    let output = run(&home, &new_json, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains '.' or '..'"));
    assert!(fs::symlink_metadata(tmp.path().join("escaped")).is_err());
}

#[test]
fn manifest_is_read_after_lock_is_acquired() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let source_a = tmp.path().join("src/a");
    let source_b = tmp.path().join("src/b");
    write_source(&source_a, "a");
    write_source(&source_b, "b");
    let target = home.join("link");
    let new_json = tmp.path().join("new.json");
    fs::write(
        &new_json,
        format!(
            r#"{{"files":[{{"target":"{}","source":"{}","clobber":false}}]}}"#,
            json_escape(&target),
            json_escape(&source_a)
        ),
    )
    .unwrap();

    let lock_dir = home.join(".local/state/manzil");
    fs::create_dir_all(&lock_dir).unwrap();
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_dir.join("lock"))
        .unwrap();
    let r = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
    assert_eq!(r, 0);

    let mut child = Command::new(env!("CARGO_BIN_EXE_manzil"))
        .env("HOME", &home)
        .arg(&new_json)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "child exited before lock release"
    );
    write_manifest(&new_json, &[(&target, &source_b, false)]);

    let r = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
    assert_eq!(r, 0);

    let output = child.wait_with_output().unwrap();
    assert_success(&output);
    assert_eq!(fs::read_link(&target).unwrap(), source_b);
}

// --- merge entries (schema v3) ---

fn merge_entry(target: &Path, patch: &Path, format: &str, clobber: bool) -> String {
    format!(
        r#"{{"type":"merge","target":"{}","source":"{}","format":"{}","clobber":{}}}"#,
        json_escape(target),
        json_escape(patch),
        format,
        clobber
    )
}

fn write_v3_manifest(path: &Path, entries: &[String]) {
    fs::write(
        path,
        format!(r#"{{"version":3,"files":[{}]}}"#, entries.join(",")),
    )
    .unwrap();
}

#[test]
fn merge_creates_missing_file() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch = tmp.path().join("patch.json");
    write_source(&patch, r#"{"editor":{"fontSize":14},"theme":"dark"}"#);
    let target = home.join(".config/app/settings.json");

    let new_json = tmp.path().join("new.json");
    write_v3_manifest(&new_json, &[merge_entry(&target, &patch, "json", false)]);

    let output = run(&home, &new_json, None);
    assert_success(&output);
    let disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(disk["editor"]["fontSize"], 14);
    assert_eq!(disk["theme"], "dark");
}

#[test]
fn merge_no_clobber_preserves_user_edit() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch = tmp.path().join("patch.json");
    write_source(&patch, r#"{"theme":"dark","fontSize":14}"#);
    let target = home.join("settings.json");
    write_source(&target, r#"{"theme":"light"}"#);

    let new_json = tmp.path().join("new.json");
    write_v3_manifest(&new_json, &[merge_entry(&target, &patch, "json", false)]);

    let output = run(&home, &new_json, None);
    assert_success(&output);
    let disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    // user's runtime value survives; missing key filled in
    assert_eq!(disk["theme"], "light");
    assert_eq!(disk["fontSize"], 14);
}

#[test]
fn merge_clobber_overwrites_existing_value() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch = tmp.path().join("patch.json");
    write_source(&patch, r#"{"theme":"dark"}"#);
    let target = home.join("settings.json");
    write_source(&target, r#"{"theme":"light","user":"alice"}"#);

    let new_json = tmp.path().join("new.json");
    write_v3_manifest(&new_json, &[merge_entry(&target, &patch, "json", true)]);

    let output = run(&home, &new_json, None);
    assert_success(&output);
    let disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(disk["theme"], "dark");
    assert_eq!(disk["user"], "alice");
}

#[test]
fn merge_prune_unmerges_owned_keys_only() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch = tmp.path().join("patch.json");
    write_source(&patch, r#"{"theme":"dark","fontSize":14}"#);
    let target = home.join("settings.json");
    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");

    // first run: merge into a file that has a pre-existing user key
    write_source(&target, r#"{"user":"alice"}"#);
    write_v3_manifest(&old_json, &[merge_entry(&target, &patch, "json", false)]);
    let output = run(&home, &old_json, None);
    assert_success(&output);

    // user edits one merged key at runtime
    let mut disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    disk["fontSize"] = serde_json::json!(18);
    fs::write(&target, serde_json::to_string(&disk).unwrap()).unwrap();

    // entry removed: theme (still ours) vanishes, fontSize (edited) and user stay
    write_v3_manifest(&new_json, &[]);
    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    let disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert!(disk.get("theme").is_none(), "owned key must be un-merged");
    assert_eq!(disk["fontSize"], 18);
    assert_eq!(disk["user"], "alice");
}

#[test]
fn merge_patch_change_converges() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch_a = tmp.path().join("patch-a.json");
    let patch_b = tmp.path().join("patch-b.json");
    write_source(&patch_a, r#"{"alpha":1}"#);
    write_source(&patch_b, r#"{"beta":2}"#);
    let target = home.join("settings.json");
    let old_json = tmp.path().join("old.json");
    let new_json = tmp.path().join("new.json");

    write_v3_manifest(&old_json, &[merge_entry(&target, &patch_a, "json", false)]);
    let output = run(&home, &old_json, None);
    assert_success(&output);

    write_v3_manifest(&new_json, &[merge_entry(&target, &patch_b, "json", false)]);
    let output = run(&home, &new_json, Some(&old_json));
    assert_success(&output);
    let disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert!(
        disk.get("alpha").is_none(),
        "key dropped from patch must not linger"
    );
    assert_eq!(disk["beta"], 2);
}

#[test]
fn merge_toml_roundtrip() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let patch = tmp.path().join("patch.json");
    write_source(&patch, r#"{"profile":{"model":"opus"}}"#);
    let target = home.join(".config/tool/config.toml");
    write_source(&target, "[profile]\nname = \"main\"\n");

    let new_json = tmp.path().join("new.json");
    write_v3_manifest(&new_json, &[merge_entry(&target, &patch, "toml", false)]);

    let output = run(&home, &new_json, None);
    assert_success(&output);
    let disk = fs::read_to_string(&target).unwrap();
    assert!(
        disk.contains("name = \"main\""),
        "existing key lost: {disk}"
    );
    assert!(
        disk.contains("model = \"opus\""),
        "patch key missing: {disk}"
    );
}
