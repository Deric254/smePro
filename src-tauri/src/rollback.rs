//! Roll back to a previously published release. The updater plugin's
//! default behavior only ever considers versions NEWER than the one
//! currently running (see `tauri-plugin-updater`'s own default
//! version comparator: `release.version > current_version`) — by
//! design, since that's the right default for the automatic checker
//! everyone gets on every launch. Rolling backward is deliberately a
//! separate, Owner-gated, manually-triggered path, not something the
//! background checker would ever do on its own.
//!
//! This module only figures out WHICH release to roll back to and
//! hands back the manifest URL for it — actually talking to the
//! updater plugin (building a one-off `Updater` with that URL and a
//! version comparator that accepts a downgrade) has to happen in a
//! `#[tauri::command]` in lib.rs, since that plugin is only reachable
//! through the Tauri runtime, not from this HTTP-API layer. See that
//! command's own doc comment for the rest of the flow, and why the
//! signing pubkey from tauri.conf.json still applies even for this
//! path (it isn't something this module could bypass even if it
//! wanted to).

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::time::Duration;

const REPO: &str = "Deric254/smePro";

#[derive(Debug, Serialize)]
pub struct ReleaseOption {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub is_prerelease: bool,
}

/// Real, published (non-draft) releases for this repo, newest first —
/// what an Owner picks a rollback target from. Owner-gated: this is
/// app-version control, the same class of action `rbac::require_owner`
/// already covers elsewhere (reconfiguring which modules exist,
/// activating the license) — not a per-module data permission.
pub fn list_releases(conn: &Connection, user_id: &str) -> Result<Vec<ReleaseOption>> {
    crate::rbac::require_owner(conn, user_id)?;

    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=30");
    let client = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let response = client
        .get(&url)
        // GitHub's REST API rejects every request with no User-Agent
        // header (403), independent of and in addition to normal rate
        // limiting — confirmed in GitHub's own REST API documentation,
        // not assumed from the error alone.
        .set("User-Agent", "smePro-app")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow!("couldn't reach GitHub: {e}"))?;

    let body = response.into_string().map_err(|e| anyhow!("failed to read GitHub's response: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| anyhow!("invalid response from GitHub: {e}"))?;
    let releases = parsed.as_array().ok_or_else(|| anyhow!("unexpected response shape from GitHub"))?;

    Ok(releases
        .iter()
        // Drafts have no public download URLs yet (still being
        // assembled by the release workflow) — never a valid rollback
        // target.
        .filter(|r| !r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false))
        .filter_map(|r| {
            Some(ReleaseOption {
                tag: r.get("tag_name")?.as_str()?.to_string(),
                name: r.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                published_at: r.get("published_at").and_then(|p| p.as_str()).unwrap_or("").to_string(),
                is_prerelease: r.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
            })
        })
        .collect())
}

/// What `check_rollback_target` confirmed about one specific tag,
/// right before the frontend is allowed to show the final "type
/// ROLLBACK to confirm" step.
#[derive(Debug, Serialize)]
pub struct RollbackCheck {
    pub manifest_url: String,
    pub target_schema_version: i32,
    pub current_schema_version: i32,
}

/// Confirms a tag is actually safe to install before the frontend ever
/// offers to — two independent things can silently go wrong with a
/// naive "just build the URL and hope" approach, and both are checked
/// for real here rather than assumed:
///
/// 1. SCHEMA COMPATIBILITY. This app's own migrations (db_migrations.rs)
///    are forward-only: an old build has no idea what a newer one did
///    to the database (some past migrations here don't just add a
///    column, they rebuild a table under a new shape entirely — see
///    db_migrations.rs's own guard for why that matters). So this
///    fetches db_migrations.rs AS IT EXISTED AT THAT TAG from GitHub
///    and reads its real `CURRENT_VERSION` constant — not a locally
///    stored guess about what that release "should" understand — and
///    compares it against this database's actual current schema
///    version. A tag whose own code understood less than what's
///    already been applied here is refused, with the specific numbers,
///    before anything installs.
///
/// 2. MANIFEST EXISTENCE. The release workflow attaches a
///    self-referencing `latest.json` to every release it cuts (see
///    .github/workflows/release.yml's tagName input) — but this
///    confirms that asset actually exists for this one chosen tag
///    rather than assuming every historical release has one (a release
///    cut by hand outside this pipeline, or one old enough to predate
///    it, wouldn't).
///
/// Either failure is Owner-visible and specific, not a generic
/// "rollback failed" — and either way, nothing is installed and the
/// app is never relaunched, so there's no window where a bad choice
/// here could leave the app mid-update.
pub fn check_rollback_target(conn: &Connection, user_id: &str, tag: &str) -> Result<RollbackCheck> {
    crate::rbac::require_owner(conn, user_id)?;

    // A tag is a git ref name the frontend already got verbatim from
    // GitHub's own release list — but it still ends up in two URL path
    // segments below, so reject anything that isn't the plain
    // "v1.2.3"-shaped tag every release this workflow has ever cut
    // actually uses, rather than trust that shape without checking it.
    let valid = tag.len() > 1
        && tag.starts_with('v')
        && tag[1..].chars().all(|c| c.is_ascii_digit() || c == '.');
    if !valid {
        return Err(anyhow!("'{tag}' isn't a valid release tag"));
    }

    let current_schema_version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
        [],
        |r| r.get(0),
    )?;

    let client = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();

    // What schema version did the code actually shipped in that tag
    // understand? Read from that tag's own source file, not guessed —
    // a stale local assumption here is exactly the kind of silently-
    // wrong shortcut this whole check exists to rule out.
    let migrations_url = format!("https://raw.githubusercontent.com/{REPO}/{tag}/src-tauri/src/db_migrations.rs");
    let source = client
        .get(&migrations_url)
        .call()
        .map_err(|_| anyhow!(
            "couldn't verify {tag}'s database compatibility right now (GitHub unreachable, or \
             this release predates this app's schema-versioning system) — rollback is blocked \
             until this can be confirmed, rather than risk it silently"
        ))?
        .into_string()
        .map_err(|e| anyhow!("couldn't read {tag}'s source to verify compatibility: {e}"))?;

    let target_schema_version = parse_current_version(&source).ok_or_else(|| {
        anyhow!("couldn't determine {tag}'s database compatibility from its source — rollback is blocked")
    })?;

    if target_schema_version < current_schema_version {
        return Err(anyhow!(
            "{tag} was built for database schema version {target_schema_version}, but this \
             business's database is already at version {current_schema_version} (from having \
             used a newer version since). Rolling back to {tag} isn't safe — that build \
             wouldn't understand the more recent database changes. Update to the latest \
             version instead, or choose a release published after this database last changed \
             shape."
        ));
    }

    // Confirmed the code would be compatible — now confirm the release
    // actually shipped an installable manifest for this platform. A
    // HEAD request is enough: this only needs to know the asset
    // exists, not its contents (the updater plugin itself validates
    // and signature-checks the real contents at install time).
    let manifest_url = format!("https://github.com/{REPO}/releases/download/{tag}/latest.json");
    client
        .head(&manifest_url)
        .call()
        .map_err(|_| anyhow!("{tag} has no installable update manifest for this platform"))?;

    Ok(RollbackCheck { manifest_url, target_schema_version, current_schema_version })
}

/// Pulls the integer out of `const CURRENT_VERSION: i32 = N;` from a
/// fetched copy of db_migrations.rs. A hand-rolled scan rather than a
/// regex dependency — the source format this reads is one line this
/// same codebase's own tests (see db_migrations.rs's CURRENT_VERSION
/// declaration) already keep in sync with reality, so this only needs
/// to parse that one known shape, not arbitrary Rust source.
fn parse_current_version(source: &str) -> Option<i32> {
    let needle = "CURRENT_VERSION: i32 = ";
    let start = source.find(needle)? + needle.len();
    let rest = &source[start..];
    let end = rest.find(';')?;
    rest[..end].trim().parse::<i32>().ok()
}
