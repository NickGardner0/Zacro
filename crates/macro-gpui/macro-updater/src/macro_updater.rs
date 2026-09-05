use anyhow::{Context as _, Result, anyhow, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    process::Command,
};
use url::Url;

pub const BUNDLE_IDENTIFIER: &str = "com.nickgardner.zacro";
const UPDATE_DIRECTORY: &str = ".zacro-update";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub version: Version,
    pub build: String,
    pub bundle_identifier: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub team_identifier: Option<String>,
    #[serde(default)]
    pub minimum_system_version: Option<String>,
}

impl UpdateManifest {
    pub fn parse(json: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(json).context("invalid update manifest JSON")?;
        manifest.validate_shape()?;
        Ok(manifest)
    }

    pub fn validate_shape(&self) -> Result<()> {
        if self.schema_version != 2 {
            bail!("unsupported update manifest schema {}", self.schema_version);
        }
        if self.bundle_identifier != BUNDLE_IDENTIFIER {
            bail!("update manifest is for a different application");
        }
        if self.build.trim().is_empty() {
            bail!("update manifest has no build identifier");
        }
        if self.size == 0 {
            bail!("update archive size must be non-zero");
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("update archive SHA-256 is malformed");
        }
        if let Some(team_identifier) = &self.team_identifier
            && (team_identifier.is_empty()
                || !team_identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric()))
        {
            bail!("update signing team identifier is malformed");
        }
        Ok(())
    }

    /// Validates the release identity against a trust root compiled into the
    /// installed application. The manifest's team identifier is descriptive
    /// metadata, not authority: a compromised update feed must not be able to
    /// replace it with an attacker's Developer ID team and have that identity
    /// accepted during bundle verification.
    pub fn validate_release_identity(&self, expected_team_identifier: &str) -> Result<()> {
        self.validate_shape()?;
        if expected_team_identifier.is_empty()
            || !expected_team_identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
        {
            bail!("the installed application has no valid pinned Developer ID team");
        }
        if self.team_identifier.as_deref() != Some(expected_team_identifier) {
            bail!("update manifest was issued for an unexpected Developer ID team");
        }
        Ok(())
    }

    pub fn is_newer_than(&self, current: &Version) -> bool {
        self.version > *current
    }

    pub fn archive_url(&self, manifest_url: &Url) -> Result<Url> {
        manifest_url
            .join(&self.url)
            .context("update archive URL is invalid")
    }
}

pub fn verify_archive(path: &Path, manifest: &UpdateManifest) -> Result<()> {
    manifest.validate_shape()?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("could not read update archive {}", path.display()))?;
    if metadata.len() != manifest.size {
        bail!(
            "update archive length mismatch: expected {}, received {}",
            manifest.size,
            metadata.len()
        );
    }
    let file = fs::File::open(path)
        .with_context(|| format!("could not open update archive {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if !actual.eq_ignore_ascii_case(&manifest.sha256) {
        bail!("update archive SHA-256 mismatch");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedUpdate {
    pub manifest: UpdateManifest,
    pub archive_path: PathBuf,
}

/// Fetches a manifest and its archive on Zed's dedicated Reqwest runtime. The
/// archive is streamed to a temporary file and promoted only after its exact
/// length and digest match the manifest.
pub async fn download_update(
    manifest_url: Url,
    current_version: Version,
    expected_team_identifier: String,
    installer: UpdateInstaller,
) -> Result<Option<DownloadedUpdate>> {
    if manifest_url.scheme() != "https" {
        bail!("automatic updates require an HTTPS manifest URL");
    }
    reqwest_client::runtime()
        .spawn(async move {
            let client = reqwest::Client::builder()
                .user_agent("zacro-updater/0.1")
                .redirect_policy(reqwest::redirect::Policy::limited(5))
                .build()?;
            let manifest_response = client.get(manifest_url.clone()).send().await?;
            if manifest_response.url().scheme() != "https" {
                bail!("update manifest redirected to an insecure URL");
            }
            if !manifest_response.status().is_success() {
                bail!(
                    "update manifest request failed with HTTP {}",
                    manifest_response.status()
                );
            }
            if manifest_response
                .content_length()
                .is_some_and(|size| size > 1024 * 1024)
            {
                bail!("update manifest exceeds the one-megabyte safety limit");
            }
            let manifest_text = manifest_response.text().await?;
            let manifest = UpdateManifest::parse(&manifest_text)?;
            manifest.validate_release_identity(&expected_team_identifier)?;
            if !manifest.is_newer_than(&current_version) {
                return Ok(None);
            }
            let archive_url = manifest.archive_url(&manifest_url)?;
            if archive_url.scheme() != "https" {
                bail!("automatic update archives must use HTTPS");
            }

            installer.prepare_state_directory()?;
            let destination = installer.archive_path();
            let partial = destination.with_extension("zip.part");
            let _ = fs::remove_file(&partial);
            let response = client.get(archive_url).send().await?;
            if response.url().scheme() != "https" {
                bail!("update archive redirected to an insecure URL");
            }
            if !response.status().is_success() {
                bail!(
                    "update archive request failed with HTTP {}",
                    response.status()
                );
            }
            if response
                .content_length()
                .is_some_and(|size| size != manifest.size)
            {
                bail!("update archive Content-Length does not match its manifest");
            }

            let download_result = async {
                use futures::StreamExt as _;
                let mut file = fs::File::create(&partial)?;
                let mut stream = response.bytes_stream();
                let mut received = 0_u64;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    received = received
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(|| anyhow!("update archive length overflow"))?;
                    if received > manifest.size {
                        bail!("update archive exceeded its declared length");
                    }
                    file.write_all(&chunk)?;
                }
                file.sync_all()?;
                verify_archive(&partial, &manifest)?;
                if destination.exists() {
                    fs::remove_file(&destination)?;
                }
                fs::rename(&partial, &destination)?;
                Ok::<_, anyhow::Error>(DownloadedUpdate {
                    manifest,
                    archive_path: destination,
                })
            }
            .await;
            if download_result.is_err() {
                let _ = fs::remove_file(&partial);
            }
            download_result.map(Some)
        })
        .await
        .context("update HTTP runtime failed")?
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
enum InstallJournal {
    Prepared {
        version: Version,
    },
    CurrentMoved {
        version: Version,
    },
    AwaitingHealthCheck {
        version: Version,
        launch_attempts: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchRecovery {
    NoPendingUpdate,
    HealthCheckRequired { version: Version },
    RolledBack { failed_version: Version },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleVerification<'a> {
    Development,
    DeveloperId { team_identifier: &'a str },
}

/// Transactional macOS bundle installer. Its state directory lives beside the
/// application so every rename is on the same filesystem. The previous bundle
/// remains intact until the new process explicitly passes its launch health
/// check.
#[derive(Debug, Clone)]
pub struct UpdateInstaller {
    app_path: PathBuf,
    state_root: PathBuf,
}

impl UpdateInstaller {
    pub fn new(app_path: impl Into<PathBuf>) -> Result<Self> {
        let app_path = app_path.into();
        let parent = app_path
            .parent()
            .ok_or_else(|| anyhow!("application bundle has no parent directory"))?;
        Ok(Self {
            state_root: parent.join(UPDATE_DIRECTORY),
            app_path,
        })
    }

    pub fn staging_path(&self) -> PathBuf {
        self.state_root.join("staged.app")
    }

    pub fn app_path(&self) -> &Path {
        &self.app_path
    }

    pub fn archive_path(&self) -> PathBuf {
        self.state_root.join("update.zip")
    }

    fn backup_path(&self) -> PathBuf {
        self.state_root.join("previous.app")
    }

    fn failed_path(&self) -> PathBuf {
        self.state_root.join("failed.app")
    }

    fn journal_path(&self) -> PathBuf {
        self.state_root.join("journal.json")
    }

    pub fn prepare_state_directory(&self) -> Result<()> {
        fs::create_dir_all(&self.state_root).with_context(|| {
            format!(
                "could not create update state directory {}",
                self.state_root.display()
            )
        })
    }

    pub fn from_current_executable() -> Result<Option<Self>> {
        let executable = std::env::current_exe().context("could not locate running executable")?;
        let Some(bundle) = executable
            .ancestors()
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        else {
            return Ok(None);
        };
        Self::new(bundle).map(Some)
    }

    pub fn stage_archive(&self, manifest: &UpdateManifest) -> Result<()> {
        verify_archive(&self.archive_path(), manifest)?;
        self.prepare_state_directory()?;
        stage_macos_archive(self)
    }

    pub fn verify_staged_bundle(&self, verification: BundleVerification<'_>) -> Result<()> {
        let staged = self.staging_path();
        if !staged.is_dir() {
            bail!("staged update does not contain an application bundle");
        }
        let plist_path = staged.join("Contents/Info.plist");
        let plist = fs::read_to_string(&plist_path)
            .with_context(|| format!("could not read {}", plist_path.display()))?;
        if !plist.contains(&format!("<string>{BUNDLE_IDENTIFIER}</string>")) {
            bail!("staged update bundle identifier does not match Zacro");
        }
        let executable = staged.join("Contents/MacOS/Zacro");
        if !executable.is_file() {
            bail!("staged update has no Zacro executable");
        }
        if let BundleVerification::DeveloperId { team_identifier } = verification {
            verify_developer_id_bundle(&staged, team_identifier)?;
        }
        Ok(())
    }

    pub fn install(
        &self,
        manifest: &UpdateManifest,
        verification: BundleVerification<'_>,
    ) -> Result<()> {
        manifest.validate_shape()?;
        if self.journal_path().exists() || self.backup_path().exists() {
            bail!("a previous update is still awaiting recovery or confirmation");
        }
        self.prepare_state_directory()?;
        self.verify_staged_bundle(verification)?;
        if !self.app_path.is_dir() {
            bail!("current application bundle does not exist");
        }

        self.write_journal(&InstallJournal::Prepared {
            version: manifest.version.clone(),
        })?;
        fs::rename(&self.app_path, self.backup_path())
            .context("could not preserve the current application bundle")?;
        self.write_journal(&InstallJournal::CurrentMoved {
            version: manifest.version.clone(),
        })?;

        if let Err(error) = fs::rename(self.staging_path(), &self.app_path) {
            let rollback = fs::rename(self.backup_path(), &self.app_path);
            if rollback.is_ok() {
                let _ = fs::remove_file(self.journal_path());
                return Err(error).context("could not activate update; rollback succeeded");
            }
            return Err(error).context("could not activate update and rollback failed");
        }
        self.write_journal(&InstallJournal::AwaitingHealthCheck {
            version: manifest.version.clone(),
            launch_attempts: 0,
        })?;
        Ok(())
    }

    /// Call at process startup. The first launch receives a health-check lease;
    /// if that process crashes before confirming it, the next launch restores
    /// the previous bundle.
    pub fn register_launch(&self) -> Result<LaunchRecovery> {
        let Some(journal) = self.read_journal()? else {
            return Ok(LaunchRecovery::NoPendingUpdate);
        };
        match journal {
            InstallJournal::Prepared { .. } => {
                self.discard_staging()?;
                self.remove_journal()?;
                Ok(LaunchRecovery::NoPendingUpdate)
            }
            InstallJournal::CurrentMoved { version } => {
                self.restore_previous_bundle()?;
                Ok(LaunchRecovery::RolledBack {
                    failed_version: version,
                })
            }
            InstallJournal::AwaitingHealthCheck {
                version,
                launch_attempts: 0,
            } => {
                self.write_journal(&InstallJournal::AwaitingHealthCheck {
                    version: version.clone(),
                    launch_attempts: 1,
                })?;
                Ok(LaunchRecovery::HealthCheckRequired { version })
            }
            InstallJournal::AwaitingHealthCheck { version, .. } => {
                self.restore_previous_bundle()?;
                Ok(LaunchRecovery::RolledBack {
                    failed_version: version,
                })
            }
        }
    }

    pub fn confirm_launch(&self) -> Result<()> {
        let Some(InstallJournal::AwaitingHealthCheck { .. }) = self.read_journal()? else {
            bail!("there is no launched update to confirm");
        };
        if !self.app_path.is_dir() {
            bail!("updated application bundle disappeared before confirmation");
        }
        remove_dir_if_present(&self.backup_path())?;
        self.remove_journal()?;
        self.discard_staging()?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        if !self.backup_path().is_dir() {
            bail!("there is no previous application bundle to restore");
        }
        self.restore_previous_bundle()
    }

    fn restore_previous_bundle(&self) -> Result<()> {
        let previous = self.backup_path();
        if !previous.is_dir() {
            bail!("update recovery has no previous application bundle");
        }
        let failed = self.failed_path();
        remove_dir_if_present(&failed)?;
        if self.app_path.exists() {
            fs::rename(&self.app_path, &failed)
                .context("could not quarantine the failed application bundle")?;
        }
        if let Err(error) = fs::rename(&previous, &self.app_path) {
            if failed.exists() {
                let _ = fs::rename(&failed, &self.app_path);
            }
            return Err(error).context("could not restore the previous application bundle");
        }
        remove_dir_if_present(&failed)?;
        self.remove_journal()?;
        self.discard_staging()?;
        Ok(())
    }

    fn discard_staging(&self) -> Result<()> {
        remove_dir_if_present(&self.staging_path())
    }

    fn read_journal(&self) -> Result<Option<InstallJournal>> {
        let path = self.journal_path();
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("could not read update journal"),
        };
        serde_json::from_str(&source)
            .map(Some)
            .context("update journal is corrupt")
    }

    fn write_journal(&self, journal: &InstallJournal) -> Result<()> {
        self.prepare_state_directory()?;
        let path = self.journal_path();
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec(journal)?)
            .context("could not write update journal")?;
        if path.exists() {
            fs::remove_file(&path).context("could not replace update journal")?;
        }
        fs::rename(&temporary, &path).context("could not commit update journal")
    }

    fn remove_journal(&self) -> Result<()> {
        match fs::remove_file(self.journal_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("could not remove update journal"),
        }
    }
}

#[cfg(target_os = "macos")]
fn verify_developer_id_bundle(bundle: &Path, team_identifier: &str) -> Result<()> {
    let verify = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(bundle)
        .output()
        .context("could not run codesign")?;
    if !verify.status.success() {
        bail!(
            "staged update code signature is invalid: {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        );
    }
    let details = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(bundle)
        .output()
        .context("could not inspect update code signature")?;
    let details = String::from_utf8_lossy(&details.stderr);
    if !details
        .lines()
        .any(|line| line.trim() == format!("Identifier={BUNDLE_IDENTIFIER}"))
    {
        bail!("staged update code signature has the wrong bundle identifier");
    }
    if !details
        .lines()
        .any(|line| line.trim() == format!("TeamIdentifier={team_identifier}"))
    {
        bail!("staged update was signed by an unexpected developer team");
    }
    let assessment = Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute", "--verbose"])
        .arg(bundle)
        .output()
        .context("could not run Gatekeeper assessment")?;
    if !assessment.status.success() {
        bail!(
            "staged update failed Gatekeeper assessment: {}",
            String::from_utf8_lossy(&assessment.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_developer_id_bundle(_bundle: &Path, _team_identifier: &str) -> Result<()> {
    bail!("Developer ID bundle verification is only available on macOS")
}

#[cfg(target_os = "macos")]
fn stage_macos_archive(installer: &UpdateInstaller) -> Result<()> {
    let extraction = installer.state_root.join("extracted");
    remove_dir_if_present(&extraction)?;
    remove_dir_if_present(&installer.staging_path())?;
    fs::create_dir_all(&extraction).context("could not create update extraction directory")?;
    let output = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(installer.archive_path())
        .arg(&extraction)
        .output()
        .context("could not run the macOS archive extractor")?;
    if !output.status.success() {
        let _ = remove_dir_if_present(&extraction);
        bail!(
            "could not extract update archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let conventional = extraction.join("Zacro.app");
    let candidate = if conventional.is_dir() {
        conventional
    } else {
        let mut apps = fs::read_dir(&extraction)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.is_dir() && path.extension().is_some_and(|extension| extension == "app")
            });
        let candidate = apps
            .next()
            .ok_or_else(|| anyhow!("update archive contains no application bundle"))?;
        if apps.next().is_some() {
            bail!("update archive contains multiple application bundles");
        }
        candidate
    };
    fs::rename(&candidate, installer.staging_path())
        .context("could not move extracted update into staging")?;
    remove_dir_if_present(&extraction)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn stage_macos_archive(_installer: &UpdateInstaller) -> Result<()> {
    bail!("Zacro .app archive staging is only available on macOS")
}

fn remove_dir_if_present(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, archive: &[u8]) -> UpdateManifest {
        UpdateManifest {
            schema_version: 2,
            version: Version::parse(version).unwrap(),
            build: "42".into(),
            bundle_identifier: BUNDLE_IDENTIFIER.into(),
            url: "Zacro-2.0.0-macos.zip".into(),
            sha256: format!("{:x}", Sha256::digest(archive)),
            size: archive.len() as u64,
            team_identifier: Some("ABCD123456".into()),
            minimum_system_version: Some("12.0".into()),
        }
    }

    fn make_bundle(path: &Path, marker: &str) {
        fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        fs::write(
            path.join("Contents/Info.plist"),
            format!("<plist><string>{BUNDLE_IDENTIFIER}</string></plist>"),
        )
        .unwrap();
        let mut executable = fs::File::create(path.join("Contents/MacOS/Zacro")).unwrap();
        executable.write_all(marker.as_bytes()).unwrap();
    }

    fn marker(path: &Path) -> String {
        fs::read_to_string(path.join("Contents/MacOS/Zacro")).unwrap()
    }

    #[test]
    fn manifest_validation_and_relative_url_resolution() {
        let manifest = manifest("2.0.0", b"archive");
        manifest.validate_shape().unwrap();
        assert!(manifest.is_newer_than(&Version::new(1, 9, 0)));
        assert_eq!(
            manifest
                .archive_url(&Url::parse("https://updates.example/macos/manifest.json").unwrap())
                .unwrap()
                .as_str(),
            "https://updates.example/macos/Zacro-2.0.0-macos.zip"
        );

        let mut wrong_app = manifest.clone();
        wrong_app.bundle_identifier = "example.attacker".into();
        assert!(wrong_app.validate_shape().is_err());
        let mut malformed_digest = manifest;
        malformed_digest.sha256 = "not-a-digest".into();
        assert!(malformed_digest.validate_shape().is_err());
    }

    #[test]
    fn release_identity_is_pinned_outside_the_downloaded_manifest() {
        let mut manifest = manifest("2.0.0", b"archive");
        manifest.validate_release_identity("ABCD123456").unwrap();
        assert!(manifest.validate_release_identity("ATTACKER123").is_err());
        manifest.team_identifier = None;
        assert!(manifest.validate_release_identity("ABCD123456").is_err());
    }

    #[test]
    fn archive_requires_exact_size_and_digest() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"signed update bytes";
        let archive = temp.path().join("update.zip");
        fs::write(&archive, bytes).unwrap();
        let manifest = manifest("2.0.0", bytes);
        verify_archive(&archive, &manifest).unwrap();

        fs::write(&archive, b"tampered update byte").unwrap();
        assert!(verify_archive(&archive, &manifest).is_err());
    }

    #[test]
    fn update_is_confirmed_only_after_a_healthy_launch() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Zacro.app");
        make_bundle(&app, "old");
        let installer = UpdateInstaller::new(&app).unwrap();
        installer.prepare_state_directory().unwrap();
        make_bundle(&installer.staging_path(), "new");
        let manifest = manifest("2.0.0", b"archive");

        installer
            .install(&manifest, BundleVerification::Development)
            .unwrap();
        assert_eq!(marker(&app), "new");
        assert_eq!(marker(&installer.backup_path()), "old");
        assert_eq!(
            installer.register_launch().unwrap(),
            LaunchRecovery::HealthCheckRequired {
                version: Version::new(2, 0, 0)
            }
        );
        installer.confirm_launch().unwrap();
        assert!(!installer.backup_path().exists());
        assert_eq!(
            installer.register_launch().unwrap(),
            LaunchRecovery::NoPendingUpdate
        );
    }

    #[test]
    fn second_unconfirmed_launch_rolls_back() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Zacro.app");
        make_bundle(&app, "old");
        let installer = UpdateInstaller::new(&app).unwrap();
        installer.prepare_state_directory().unwrap();
        make_bundle(&installer.staging_path(), "new");
        let manifest = manifest("2.0.0", b"archive");
        installer
            .install(&manifest, BundleVerification::Development)
            .unwrap();

        assert!(matches!(
            installer.register_launch().unwrap(),
            LaunchRecovery::HealthCheckRequired { .. }
        ));
        assert_eq!(
            installer.register_launch().unwrap(),
            LaunchRecovery::RolledBack {
                failed_version: Version::new(2, 0, 0)
            }
        );
        assert_eq!(marker(&app), "old");
        assert!(!installer.backup_path().exists());
    }

    #[test]
    fn interrupted_swap_recovers_previous_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Zacro.app");
        make_bundle(&app, "old");
        let installer = UpdateInstaller::new(&app).unwrap();
        installer.prepare_state_directory().unwrap();
        fs::rename(&app, installer.backup_path()).unwrap();
        installer
            .write_journal(&InstallJournal::CurrentMoved {
                version: Version::new(2, 0, 0),
            })
            .unwrap();

        assert_eq!(
            installer.register_launch().unwrap(),
            LaunchRecovery::RolledBack {
                failed_version: Version::new(2, 0, 0)
            }
        );
        assert_eq!(marker(&app), "old");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_zip_is_verified_and_staged_with_bundle_resources() {
        let temp = tempfile::tempdir().unwrap();
        let installed = temp.path().join("Applications/Zacro.app");
        make_bundle(&installed, "old");
        let source = temp.path().join("release/Zacro.app");
        make_bundle(&source, "new");
        fs::write(source.join("Contents/Resources.txt"), "resource").unwrap();
        let installer = UpdateInstaller::new(&installed).unwrap();
        installer.prepare_state_directory().unwrap();
        let output = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(&source)
            .arg(installer.archive_path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let bytes = fs::read(installer.archive_path()).unwrap();
        let manifest = manifest("2.0.0", &bytes);

        installer.stage_archive(&manifest).unwrap();
        installer
            .verify_staged_bundle(BundleVerification::Development)
            .unwrap();
        assert_eq!(marker(&installer.staging_path()), "new");
        assert_eq!(
            fs::read_to_string(installer.staging_path().join("Contents/Resources.txt")).unwrap(),
            "resource"
        );
    }
}
