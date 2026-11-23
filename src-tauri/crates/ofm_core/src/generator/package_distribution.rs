//! Opt-in distribution gate. Never called by package loading or authoring validation.
use super::package::{
    MAX_ARCHIVE_BYTES, MAX_FILE_COUNT, MAX_UNCOMPRESSED_BYTES, RecordDataKind, RecordEntityKind,
    RecordProvenanceDef, RecordRightsStatus, WorldPackage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A review supplied by the distributor's trusted registry, never by the package author.
/// The caller is responsible for authenticating the registry and its reviewers. This type
/// deliberately has no package deserialization path. Pass only this package's review records;
/// extraneous records, including reviews for another package, fail closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedRightsReview {
    pub package_id: String,
    pub entity_kind: RecordEntityKind,
    pub entity_id: String,
    pub provenance_sha256: String,
    pub record_sha256: String,
    pub status: RecordRightsStatus,
    pub reviewer: String,
    /// Strict ISO calendar date, YYYY-MM-DD.
    pub reviewed_at: String,
    pub evidence_url: String,
}

/// A separately controlled JSON registry passed to distribution tooling. This is not package
/// metadata; callers must source it from a trusted, independently reviewed location.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedRightsReviewRegistry {
    pub format_version: u32,
    pub reviews: Vec<TrustedRightsReview>,
}

pub const TRUSTED_RIGHTS_REVIEW_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributionIssue {
    pub entity_kind: RecordEntityKind,
    pub entity_id: String,
    pub code: &'static str,
}

/// Versioned digest of the complete typed provenance record, including the author's rights
/// claim and every provider identity. Normalization is deserialization into RecordProvenanceDef
/// (defaults and canonical field order), then compact JSON. Strings and array order are retained
/// exactly: even whitespace edits invalidate approval. No metadata is filtered from the hash.
pub fn provenance_fingerprint(record: &RecordProvenanceDef) -> Result<String, serde_json::Error> {
    let mut hash = Sha256::new();
    hash.update(b"openfoot-provenance-v1\0");
    hash.update(serde_json::to_vec(record)?);
    Ok(hex::encode(hash.finalize()))
}

/// Canonical JSON sorts every object recursively, including maps inside name pools.
fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            let mut bytes = vec![b'{'];
            for (i, (key, value)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    bytes.push(b',');
                }
                bytes.extend(serde_json::to_vec(key)?);
                bytes.push(b':');
                bytes.extend(canonical_json(value)?);
            }
            bytes.push(b'}');
            Ok(bytes)
        }
        serde_json::Value::Array(values) => {
            let mut bytes = vec![b'['];
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    bytes.push(b',');
                }
                bytes.extend(canonical_json(value)?);
            }
            bytes.push(b']');
            Ok(bytes)
        }
        _ => serde_json::to_vec(value),
    }
}

/// Bind an approval to a unique typed record and all manifest metadata except provenance.
/// Missing/ambiguous records fail; array order and string contents are significant.
fn record_fingerprint(
    package: &WorldPackage,
    kind: RecordEntityKind,
    id: &str,
) -> Result<String, serde_json::Error> {
    use serde::ser::Error;
    let invalid = || serde_json::Error::custom("missing or ambiguous distribution record");
    let meta = package.meta.as_ref().ok_or_else(invalid)?;
    if package_records(package)
        .iter()
        .filter(|&&(k, i)| k == kind && i == id)
        .count()
        != 1
        || (kind == RecordEntityKind::Names
            && package.repeated_name_pools.iter().any(|p| p.country == id))
    {
        return Err(invalid());
    }
    let mut manifest = serde_json::to_value(meta)?;
    if let Some(map) = manifest.as_object_mut() {
        map.remove("recordProvenance");
    }
    macro_rules! selected {
        ($records:expr) => {
            serde_json::to_value($records.iter().find(|r| r.id == id).ok_or_else(invalid)?)?
        };
    }
    let payload = match kind {
        RecordEntityKind::World => serde_json::json!({
            "manifest": manifest,
            "namesDefinition": package.names.as_ref().map(|names| {
                serde_json::json!({"version": names.version, "description": names.description})
            }),
        }),
        RecordEntityKind::Player => selected!(package.players),
        RecordEntityKind::Staff => selected!(package.staff),
        RecordEntityKind::Team => selected!(package.teams),
        RecordEntityKind::Country => selected!(package.countries),
        RecordEntityKind::Confederation => selected!(package.confederations),
        RecordEntityKind::Competition => selected!(package.competitions),
        RecordEntityKind::Names => {
            let names = package.names.as_ref().ok_or_else(invalid)?;
            let pool = names.pools.get(id).ok_or_else(invalid)?;
            serde_json::json!({
                "version": names.version,
                "description": names.description,
                "pool": pool,
            })
        }
        RecordEntityKind::Unknown => return Err(invalid()),
    };
    let envelope = serde_json::json!([meta.id, kind, id, manifest, payload]);
    let mut hash = Sha256::new();
    hash.update(b"openfoot-record-v1\0");
    hash.update(canonical_json(&envelope)?);
    Ok(hex::encode(hash.finalize()))
}

/// Public digest contract for distribution tooling; errors never panic.
pub fn record_payload_fingerprint(
    package: &WorldPackage,
    entity_kind: RecordEntityKind,
    entity_id: &str,
) -> Result<String, String> {
    record_fingerprint(package, entity_kind, entity_id).map_err(|error| error.to_string())
}

const MAX_SCHEMA_BYTES: u64 = 16 * 1024 * 1024;

fn file_issue(issues: &mut Vec<DistributionIssue>, path: &str, code: &'static str) {
    issue(issues, RecordEntityKind::World, path, code);
}

fn safe_inventory_path(path: &str) -> bool {
    path.is_ascii()
        && !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.ends_with(['.', ' '])
        })
}

fn inspect_schema_file(
    path: &str,
    reader: impl std::io::Read,
    issues: &mut Vec<DistributionIssue>,
) -> u64 {
    use std::io::Read;
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str());
    let is_translation = file_name.to_ascii_lowercase().starts_with("translations.");
    let has_supported_extension = extension.is_some_and(|extension| {
        ["json", "yaml", "yml"]
            .iter()
            .any(|supported| extension.eq_ignore_ascii_case(supported))
    });
    if is_translation || !has_supported_extension {
        file_issue(issues, path, "unsupported_distribution_file");
        return 0;
    }
    let mut bytes = Vec::new();
    if reader
        .take(MAX_SCHEMA_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        file_issue(issues, path, "unreadable_distribution_file");
        return bytes.len() as u64;
    }
    if bytes.len() as u64 > MAX_SCHEMA_BYTES {
        file_issue(issues, path, "oversized_distribution_file");
        return bytes.len() as u64;
    }
    let value: Result<serde_json::Value, String> =
        if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("json")) {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())
        } else {
            serde_yaml::from_slice(&bytes).map_err(|e| e.to_string())
        };
    match value {
        Ok(value)
            if matches!(
                value.get("schema").and_then(|s| s.as_str()),
                Some(
                    "world"
                        | "team"
                        | "player"
                        | "staff"
                        | "confederation"
                        | "country"
                        | "competition"
                        | "names"
                )
            ) => {}
        Ok(_) => file_issue(issues, path, "unsupported_distribution_file"),
        Err(_) => file_issue(issues, path, "unreadable_distribution_file"),
    }
    bytes.len() as u64
}

/// Fail-closed inventory only; structural package validation must also run.
/// File issues use entity_id as the slash-separated relative path ("." for root errors).
/// Symlinks and special files are never followed. This does not change authoring/loading.
pub fn validate_distribution_directory_files(root: &std::path::Path) -> Vec<DistributionIssue> {
    validate_distribution_directory_files_with_limits(root, MAX_FILE_COUNT, MAX_UNCOMPRESSED_BYTES)
}

fn validate_distribution_directory_files_with_limits(
    root: &std::path::Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Vec<DistributionIssue> {
    let mut issues = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    'walk: while let Some((directory, relative)) = pending.pop() {
        if !std::fs::symlink_metadata(&directory)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            file_issue(
                &mut issues,
                if relative.is_empty() { "." } else { &relative },
                "unreadable_distribution_file",
            );
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                file_issue(
                    &mut issues,
                    if relative.is_empty() { "." } else { &relative },
                    "unreadable_distribution_file",
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    file_issue(
                        &mut issues,
                        if relative.is_empty() { "." } else { &relative },
                        "unreadable_distribution_file",
                    );
                    continue;
                }
            };
            let name = entry.file_name();
            file_count = file_count.saturating_add(1);
            if file_count > max_files {
                file_issue(
                    &mut issues,
                    if relative.is_empty() { "." } else { &relative },
                    "too_many_distribution_files",
                );
                break 'walk;
            }
            let path = if relative.is_empty() {
                name.to_string_lossy().into_owned()
            } else {
                format!("{relative}/{}", name.to_string_lossy())
            };
            if name.to_str().is_none() || !safe_inventory_path(&path) {
                file_issue(&mut issues, &path, "unsafe_distribution_path");
                continue;
            }
            if !seen.insert(path.to_ascii_lowercase()) {
                file_issue(&mut issues, &path, "duplicate_distribution_path");
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push((entry.path(), path)),
                Ok(kind) if kind.is_file() => match std::fs::File::open(entry.path()) {
                    Ok(file) => {
                        total_bytes = total_bytes.saturating_add(inspect_schema_file(
                            &path,
                            file,
                            &mut issues,
                        ));
                        if total_bytes > max_total_bytes {
                            file_issue(&mut issues, &path, "distribution_package_too_large");
                            break 'walk;
                        }
                    }
                    Err(_) => file_issue(&mut issues, &path, "unreadable_distribution_file"),
                },
                _ => file_issue(&mut issues, &path, "unreadable_distribution_file"),
            }
        }
    }
    issues.sort_by(|a, b| (&a.entity_id, a.code).cmp(&(&b.entity_id, b.code)));
    issues
}

/// Inspect an .ofm ZIP without extraction; unsafe paths, duplicates and links fail closed.
pub fn validate_distribution_archive_files(path: &std::path::Path) -> Vec<DistributionIssue> {
    validate_distribution_archive_files_with_limits(
        path,
        MAX_FILE_COUNT,
        MAX_UNCOMPRESSED_BYTES,
        MAX_ARCHIVE_BYTES,
    )
}

fn validate_distribution_archive_files_with_limits(
    path: &std::path::Path,
    max_files: usize,
    max_total_bytes: u64,
    max_archive_bytes: u64,
) -> Vec<DistributionIssue> {
    let mut issues = Vec::new();
    let Some(mut archive) =
        open_distribution_archive(path, max_files, max_archive_bytes, &mut issues)
    else {
        return issues;
    };
    // zip indexes duplicate central-directory names into one entry; its consistency
    // check is essential even though we separately reject case/path aliases below.
    let inconsistent = archive.validate_header_consistency().is_err();
    let mut seen = std::collections::BTreeSet::new();
    let mut budget = ArchiveByteBudget {
        max_total_bytes,
        declared_total: 0,
        read_total: 0,
    };
    for index in 0..archive.len() {
        if inspect_archive_entry(
            &mut archive,
            index,
            inconsistent,
            &mut seen,
            &mut budget,
            &mut issues,
        )
        .is_break()
        {
            break;
        }
    }
    issues.sort_by(|a, b| (&a.entity_id, a.code).cmp(&(&b.entity_id, b.code)));
    issues
}

/// Opens the archive, or records why it cannot be inspected and returns `None`.
fn open_distribution_archive(
    path: &std::path::Path,
    max_files: usize,
    max_archive_bytes: u64,
    issues: &mut Vec<DistributionIssue>,
) -> Option<zip::ZipArchive<std::fs::File>> {
    let Ok(file) = std::fs::File::open(path) else {
        file_issue(issues, ".", "unreadable_distribution_archive");
        return None;
    };
    match file.metadata() {
        Ok(metadata) if !metadata.is_file() => {
            file_issue(issues, ".", "unreadable_distribution_archive");
            return None;
        }
        Ok(metadata) if metadata.len() > max_archive_bytes => {
            file_issue(issues, ".", "oversized_distribution_archive");
            return None;
        }
        Ok(_) => {}
        Err(_) => {
            file_issue(issues, ".", "unreadable_distribution_archive");
            return None;
        }
    }
    let archive = zip::ZipArchive::new(file).ok();
    let Some(archive) = archive else {
        file_issue(issues, ".", "unreadable_distribution_archive");
        return None;
    };
    if archive.len() > max_files {
        file_issue(issues, ".", "too_many_distribution_files");
        return None;
    }
    Some(archive)
}

/// Running byte totals across archive entries, checked against one shared ceiling.
struct ArchiveByteBudget {
    max_total_bytes: u64,
    declared_total: u64,
    read_total: u64,
}

/// `Break` means the inventory must stop: the package is over its uncompressed-size ceiling.
fn inspect_archive_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    index: usize,
    inconsistent: bool,
    seen: &mut std::collections::BTreeSet<String>,
    budget: &mut ArchiveByteBudget,
    issues: &mut Vec<DistributionIssue>,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow::Continue;
    let relative = archive
        .name_for_index(index)
        .and_then(Result::ok)
        .map(|name| name.into_owned())
        .unwrap_or_else(|| format!("@entry/{index}"));
    if inconsistent {
        file_issue(issues, &relative, "invalid_distribution_archive_headers");
    }
    let mut entry = match archive.by_index(index) {
        Ok(entry) => entry,
        Err(_) => {
            file_issue(issues, &relative, "unreadable_distribution_file");
            return Continue(());
        }
    };
    let name = match entry.name() {
        Ok(name) => name.into_owned(),
        Err(_) => {
            file_issue(issues, &relative, "unsafe_distribution_path");
            return Continue(());
        }
    };
    let normalized = if entry.is_dir() {
        name.strip_suffix('/').unwrap_or(&name)
    } else {
        &name
    };
    if !safe_inventory_path(normalized) {
        file_issue(issues, &name, "unsafe_distribution_path");
        return Continue(());
    }
    if !seen.insert(normalized.to_ascii_lowercase()) {
        file_issue(issues, &name, "duplicate_distribution_path");
    }
    if entry.unix_mode().is_some_and(|mode| {
        let kind = mode & 0o170000;
        kind != 0 && kind != 0o100000 && kind != 0o040000
    }) {
        file_issue(issues, &name, "unsafe_distribution_path");
        Continue(())
    } else if entry.is_dir() {
        let (declared_size, compressed_size) = (entry.size(), entry.compressed_size());
        inspect_archive_directory_entry(
            &name,
            declared_size,
            compressed_size,
            &mut entry,
            budget,
            issues,
        )
    } else {
        let declared_size = entry.size();
        inspect_archive_file_entry(&name, declared_size, &mut entry, budget, issues)
    }
}

fn inspect_archive_directory_entry(
    name: &str,
    declared_size: u64,
    compressed_size: u64,
    reader: &mut impl std::io::Read,
    budget: &mut ArchiveByteBudget,
    issues: &mut Vec<DistributionIssue>,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow::{Break, Continue};
    budget.declared_total = budget.declared_total.saturating_add(declared_size);
    if budget.declared_total > budget.max_total_bytes {
        file_issue(issues, name, "distribution_package_too_large");
        return Break(());
    }
    // Directory records are structural only. Bound the read to one byte so even a
    // forged size field cannot make this inventory decompress an arbitrary payload.
    let mut byte = [0u8; 1];
    match reader.read(&mut byte) {
        Ok(0) if declared_size == 0 && compressed_size == 0 => {}
        Ok(read) => {
            budget.read_total = budget.read_total.saturating_add(read as u64);
            file_issue(issues, name, "nonempty_distribution_directory_entry");
            if budget.read_total > budget.max_total_bytes {
                file_issue(issues, name, "distribution_package_too_large");
                return Break(());
            }
        }
        Err(_) => {
            if declared_size != 0 || compressed_size != 0 {
                file_issue(issues, name, "nonempty_distribution_directory_entry");
            }
            file_issue(issues, name, "unreadable_distribution_file");
        }
    }
    Continue(())
}

fn inspect_archive_file_entry(
    name: &str,
    declared_size: u64,
    reader: &mut impl std::io::Read,
    budget: &mut ArchiveByteBudget,
    issues: &mut Vec<DistributionIssue>,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow::{Break, Continue};
    budget.declared_total = budget.declared_total.saturating_add(declared_size);
    if budget.declared_total > budget.max_total_bytes {
        file_issue(issues, name, "distribution_package_too_large");
        return Break(());
    }
    if declared_size > MAX_SCHEMA_BYTES {
        file_issue(issues, name, "oversized_distribution_file");
    } else {
        budget.read_total = budget
            .read_total
            .saturating_add(inspect_schema_file(name, reader, issues));
        if budget.read_total > budget.max_total_bytes {
            file_issue(issues, name, "distribution_package_too_large");
            return Break(());
        }
    }
    Continue(())
}

/// Empty issues means complete provenance and externally reviewed distribution rights.
/// This gate does not replace structural validation or authenticate registry storage.
pub fn validate_package_distribution(
    package: &WorldPackage,
    trusted_reviews: &[TrustedRightsReview],
) -> Vec<DistributionIssue> {
    let mut issues = Vec::new();
    let Some(meta) = &package.meta else {
        issue(&mut issues, RecordEntityKind::World, "", "missing_manifest");
        return issues;
    };
    let records = package_records(package);
    let mut translation_locales: Vec<_> = package.extra_translations.keys().collect();
    translation_locales.sort();
    for locale in translation_locales {
        issue(
            &mut issues,
            RecordEntityKind::World,
            &format!("translations.{locale}.json"),
            "unsupported_distribution_file",
        );
    }
    let mut counts = BTreeMap::new();
    for &(kind, id) in &records {
        if id.trim().is_empty() {
            issue(&mut issues, kind, id, "blank_id");
        }
        let count = counts.entry((kind as u8, id)).or_insert(0);
        *count += 1;
        if *count > 1 {
            issue(&mut issues, kind, id, "duplicate_record");
        }
    }
    for pool in &package.repeated_name_pools {
        issue(
            &mut issues,
            RecordEntityKind::Names,
            &pool.country,
            "duplicate_record",
        );
    }
    let mut provenance_counts = BTreeMap::new();
    for p in &meta.record_provenance {
        let key = (p.entity_kind as u8, p.entity_id.as_str());
        if p.entity_id.trim().is_empty() {
            issue(&mut issues, p.entity_kind, &p.entity_id, "blank_id");
        }
        if !counts.contains_key(&key) {
            issue(
                &mut issues,
                p.entity_kind,
                &p.entity_id,
                "orphan_provenance",
            );
        }
        let count = provenance_counts.entry(key).or_insert(0);
        *count += 1;
        if *count > 1 {
            issue(
                &mut issues,
                p.entity_kind,
                &p.entity_id,
                "duplicate_provenance",
            );
        }
        validate_provenance(p, &mut issues);
    }
    for &(kind, id) in &records {
        if !provenance_counts.contains_key(&(kind as u8, id)) {
            issue(&mut issues, kind, id, "missing_provenance");
        }
    }
    validate_reviews(
        package,
        &meta.id,
        &meta.record_provenance,
        trusted_reviews,
        &mut issues,
    );
    issues
}

fn issue(
    issues: &mut Vec<DistributionIssue>,
    kind: RecordEntityKind,
    id: &str,
    code: &'static str,
) {
    issues.push(DistributionIssue {
        entity_kind: kind,
        entity_id: id.into(),
        code,
    });
}

fn package_records(package: &WorldPackage) -> Vec<(RecordEntityKind, &str)> {
    use RecordEntityKind::{
        Competition, Confederation, Country, Names, Player, Staff, Team, World,
    };
    let mut records = Vec::new();
    if let Some(meta) = &package.meta {
        records.push((World, meta.id.as_str()));
    }
    records.extend(
        package
            .confederations
            .iter()
            .map(|r| (Confederation, r.id.as_str())),
    );
    records.extend(package.countries.iter().map(|r| (Country, r.id.as_str())));
    records.extend(package.teams.iter().map(|r| (Team, r.id.as_str())));
    records.extend(package.players.iter().map(|r| (Player, r.id.as_str())));
    records.extend(package.staff.iter().map(|r| (Staff, r.id.as_str())));
    records.extend(
        package
            .competitions
            .iter()
            .map(|r| (Competition, r.id.as_str())),
    );
    if let Some(names) = &package.names {
        let mut keys: Vec<_> = names.pools.keys().collect();
        keys.sort();
        records.extend(keys.into_iter().map(|id| (Names, id.as_str())));
    }
    records
}

fn validate_provenance(p: &RecordProvenanceDef, issues: &mut Vec<DistributionIssue>) {
    if p.provider_identities.is_empty()
        || p.provider_identities.iter().any(|identity| {
            identity.provider.trim().is_empty() || identity.provider_entity_id.trim().is_empty()
        })
    {
        issue(
            issues,
            p.entity_kind,
            &p.entity_id,
            "invalid_provider_identity",
        );
    }
    for (value, code) in [
        (&p.source, "missing_source"),
        (&p.source_url, "missing_source_url"),
        (&p.as_of, "missing_as_of"),
        (&p.retrieved_at, "missing_retrieved_at"),
        (&p.license, "missing_license"),
        (&p.license_url, "missing_license_url"),
    ] {
        if value.as_deref().is_none_or(|text| text.trim().is_empty()) {
            issue(issues, p.entity_kind, &p.entity_id, code);
        }
    }
    if p.as_of
        .as_deref()
        .is_some_and(|date| !valid_review_date(date))
    {
        issue(issues, p.entity_kind, &p.entity_id, "invalid_as_of");
    }
    if p.retrieved_at
        .as_deref()
        .is_some_and(|timestamp| !valid_retrieved_at(timestamp))
    {
        issue(issues, p.entity_kind, &p.entity_id, "invalid_retrieved_at");
    }
    if p.source_url
        .as_deref()
        .is_some_and(|url| !evidence_url(url))
    {
        issue(issues, p.entity_kind, &p.entity_id, "invalid_source_url");
    }
    if p.license_url
        .as_deref()
        .is_some_and(|url| !evidence_url(url))
    {
        issue(issues, p.entity_kind, &p.entity_id, "invalid_license_url");
    }
    if p.kind == RecordDataKind::Unverified {
        issue(issues, p.entity_kind, &p.entity_id, "unverified");
    }
    if p.distribution_status == RecordRightsStatus::Restricted {
        issue(issues, p.entity_kind, &p.entity_id, "restricted");
    }
}

fn validate_reviews(
    package: &WorldPackage,
    package_id: &str,
    provenance: &[RecordProvenanceDef],
    reviews: &[TrustedRightsReview],
    issues: &mut Vec<DistributionIssue>,
) {
    let records: BTreeMap<_, _> = provenance
        .iter()
        .map(|p| ((p.entity_kind as u8, p.entity_id.as_str()), p))
        .collect();
    let mut counts = BTreeMap::new();
    for review in reviews {
        let key = (review.entity_kind as u8, review.entity_id.as_str());
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        if *count > 1 {
            issue(
                issues,
                review.entity_kind,
                &review.entity_id,
                "duplicate_review",
            );
        }
        let Some(record) = records
            .get(&key)
            .filter(|_| review.package_id == package_id)
        else {
            issue(
                issues,
                review.entity_kind,
                &review.entity_id,
                "orphan_review",
            );
            continue;
        };
        if review.status != RecordRightsStatus::Allowed
            || review.reviewer.trim().is_empty()
            || !valid_review_date(&review.reviewed_at)
            || !evidence_url(&review.evidence_url)
            || review.package_id.trim().is_empty()
            || review.entity_id.trim().is_empty()
        {
            issue(
                issues,
                review.entity_kind,
                &review.entity_id,
                "invalid_review",
            );
        }
        match provenance_fingerprint(record) {
            Ok(fingerprint)
                if fingerprint == review.provenance_sha256
                    && record_fingerprint(package, review.entity_kind, &review.entity_id)
                        .is_ok_and(|digest| digest == review.record_sha256) => {}
            _ => issue(
                issues,
                review.entity_kind,
                &review.entity_id,
                "stale_review",
            ),
        }
    }
    for p in provenance {
        if !counts.contains_key(&(p.entity_kind as u8, p.entity_id.as_str())) {
            issue(issues, p.entity_kind, &p.entity_id, "missing_review");
        }
    }
}

fn valid_review_date(text: &str) -> bool {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == text)
}

fn valid_retrieved_at(text: &str) -> bool {
    !text.ends_with("-00:00")
        && chrono::DateTime::parse_from_rfc3339(text)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

// Reject URL parser recovery (whitespace/backslashes), userinfo and missing authorities.
fn evidence_url(text: &str) -> bool {
    if text
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return false;
    }
    let Some((_, rest)) = text.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.ends_with(':') {
        return false;
    }
    url::Url::parse(text).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::super::package::{ProviderIdentityDef, WorldMetaDef, validate_package};
    use super::*;

    fn record(kind: RecordEntityKind, id: &str) -> RecordProvenanceDef {
        RecordProvenanceDef {
            entity_kind: kind,
            entity_id: id.into(),
            kind: RecordDataKind::Observed,
            provider_identities: vec![ProviderIdentityDef {
                provider: "archive".into(),
                provider_entity_id: id.into(),
            }],
            source: Some("Archive".into()),
            source_url: Some("https://example.org/data".into()),
            as_of: Some("2026-01-01".into()),
            retrieved_at: Some("2026-01-02T12:00:00Z".into()),
            license: Some("CC0-1.0".into()),
            license_url: Some("https://example.org/license".into()),
            distribution_status: RecordRightsStatus::Allowed,
        }
    }

    fn review(record: &RecordProvenanceDef) -> TrustedRightsReview {
        TrustedRightsReview {
            package_id: "sample".into(),
            entity_kind: record.entity_kind,
            entity_id: record.entity_id.clone(),
            provenance_sha256: provenance_fingerprint(record).unwrap(),
            record_sha256: String::new(),
            status: RecordRightsStatus::Allowed,
            reviewer: "independent-reviewer".into(),
            reviewed_at: "2026-01-03".into(),
            evidence_url: "https://example.org/reviews/1".into(),
        }
    }

    fn fixture() -> (WorldPackage, Vec<TrustedRightsReview>) {
        let provenance = record(RecordEntityKind::World, "sample");
        let mut reviews = vec![review(&provenance)];
        let mut package = WorldPackage::default();
        package.meta = Some(WorldMetaDef {
            id: "sample".into(),
            name: "Sample".into(),
            version: "1.0.0".into(),
            author: "Author".into(),
            license: "CC0-1.0".into(),
            package_type: "database".into(),
            format_version: 1,
            record_provenance: vec![provenance],
            ..WorldMetaDef::default()
        });
        reviews[0].record_sha256 =
            record_fingerprint(&package, RecordEntityKind::World, "sample").unwrap();
        (package, reviews)
    }

    fn provenance(package: &mut WorldPackage) -> &mut Vec<RecordProvenanceDef> {
        &mut package.meta.as_mut().unwrap().record_provenance
    }

    fn fails(package: &WorldPackage, reviews: &[TrustedRightsReview], code: &str) {
        assert!(
            validate_package_distribution(package, reviews)
                .iter()
                .any(|issue| issue.code == code),
            "expected {code}"
        );
    }

    /// Given complete reviewed provenance, when checked, then distribution is permitted.
    #[test]
    fn a_reviewed_record_passes() {
        let (package, reviews) = fixture();
        assert!(validate_package_distribution(&package, &reviews).is_empty());
    }

    /// Given missing provenance, when checked, then the uncovered manifest fails.
    #[test]
    fn missing_provenance_fails() {
        let (mut package, _) = fixture();
        provenance(&mut package).clear();
        fails(&package, &[], "missing_provenance");
    }

    /// Given duplicate provenance, when checked, then an approval cannot resolve ambiguity.
    #[test]
    fn duplicate_provenance_fails() {
        let (mut package, reviews) = fixture();
        let duplicate = provenance(&mut package)[0].clone();
        provenance(&mut package).push(duplicate);
        fails(&package, &reviews, "duplicate_provenance");
    }

    /// Given provenance for an absent player, when checked, then it is orphaned.
    #[test]
    fn orphan_provenance_fails() {
        let (mut package, reviews) = fixture();
        provenance(&mut package).push(record(RecordEntityKind::Player, "absent"));
        fails(&package, &reviews, "orphan_provenance");
    }

    /// Given incomplete provider identity, when checked, then even an exact review fails.
    #[test]
    fn blank_provider_identity_fails() {
        for field in 0..3 {
            let (mut package, _) = fixture();
            let p = &mut provenance(&mut package)[0];
            match field {
                0 => p.provider_identities.clear(),
                1 => p.provider_identities[0].provider = " ".into(),
                _ => p.provider_identities[0].provider_entity_id = " ".into(),
            }
            let reviews = vec![review(p)];
            fails(&package, &reviews, "invalid_provider_identity");
        }
    }

    /// Given a restricted author claim, when approved externally, then restriction survives.
    #[test]
    fn restricted_claims_cannot_be_overridden() {
        let (mut package, _) = fixture();
        let p = &mut provenance(&mut package)[0];
        p.distribution_status = RecordRightsStatus::Restricted;
        let reviews = vec![review(p)];
        fails(&package, &reviews, "restricted");
    }

    /// Given author-allowed provenance alone, when checked, then rights remain unreviewed.
    #[test]
    fn author_allowed_without_review_fails() {
        let (package, _) = fixture();
        fails(&package, &[], "missing_review");
    }

    /// Given a changed source, when an earlier approval is used, then it is stale.
    #[test]
    fn stale_approval_fails() {
        let (mut package, reviews) = fixture();
        provenance(&mut package)[0].source = Some("Changed".into());
        fails(&package, &reviews, "stale_review");
    }

    /// Given duplicate or misplaced registry records, when checked, then neither is ignored.
    #[test]
    fn duplicate_and_orphan_reviews_fail() {
        let (package, mut reviews) = fixture();
        reviews.push(reviews[0].clone());
        fails(&package, &reviews, "duplicate_review");
        reviews.pop();
        reviews[0].package_id = "other".into();
        fails(&package, &reviews, "orphan_review");
    }

    /// Given incomplete or invalid review evidence, when checked, then approval fails closed.
    #[test]
    fn invalid_review_evidence_fails() {
        for field in 0..5 {
            let (package, mut reviews) = fixture();
            match field {
                0 => reviews[0].reviewer = " ".into(),
                1 => reviews[0].reviewed_at = "2026-02-30".into(),
                2 => reviews[0].evidence_url = "not a URL".into(),
                3 => reviews[0].status = RecordRightsStatus::Restricted,
                _ => reviews[0].status = RecordRightsStatus::Unknown,
            }
            fails(&package, &reviews, "invalid_review");
        }
    }

    /// Given two country name pools, when one country lacks provenance, then coverage fails.
    #[test]
    fn names_pools_are_keyed_by_country() {
        let (mut package, mut reviews) = fixture();
        package.names = Some(
            serde_json::from_value(serde_json::json!({"pools": {
                "TR": {"first_names": ["Ali"], "last_names": ["Can"]},
                "GB": {"first_names": ["John"], "last_names": ["Smith"]}
            }}))
            .unwrap(),
        );
        reviews[0].record_sha256 =
            record_fingerprint(&package, RecordEntityKind::World, "sample").unwrap();
        for id in ["TR", "GB"] {
            let p = record(RecordEntityKind::Names, id);
            let mut approval = review(&p);
            approval.record_sha256 =
                record_fingerprint(&package, RecordEntityKind::Names, id).unwrap();
            reviews.push(approval);
            provenance(&mut package).push(p);
        }
        assert!(validate_package_distribution(&package, &reviews).is_empty());
        provenance(&mut package).pop();
        fails(&package, &reviews, "missing_provenance");
    }

    /// Given a legacy package, when the separate gate is used, then authoring results stay unchanged.
    #[test]
    fn legacy_validation_is_unaffected() {
        let (mut package, _) = fixture();
        provenance(&mut package).clear();
        let before = format!("{:?}", validate_package(&package));
        fails(&package, &[], "missing_provenance");
        assert_eq!(before, format!("{:?}", validate_package(&package)));
        assert!(!before.contains("provenance"));
    }

    /// Given absent or blank evidence fields, when reviewed exactly, then completeness still fails.
    #[test]
    fn every_required_evidence_field_is_checked() {
        for field in 0..6 {
            for value in [None, Some(" \t".to_string())] {
                let (mut package, _) = fixture();
                let p = &mut provenance(&mut package)[0];
                let (slot, code) = match field {
                    0 => (&mut p.source, "missing_source"),
                    1 => (&mut p.source_url, "missing_source_url"),
                    2 => (&mut p.as_of, "missing_as_of"),
                    3 => (&mut p.retrieved_at, "missing_retrieved_at"),
                    4 => (&mut p.license, "missing_license"),
                    _ => (&mut p.license_url, "missing_license_url"),
                };
                *slot = value;
                let reviews = vec![review(p)];
                fails(&package, &reviews, code);
            }
        }
    }

    /// Given malformed dates or non-HTTP source links, when checked, then the release gate rejects them.
    #[test]
    fn dates_and_source_links_follow_the_documented_formats() {
        for (field, code, value) in [
            (0, "invalid_as_of", "2026-02-30"),
            (1, "invalid_retrieved_at", "2026-01-02"),
            (1, "invalid_retrieved_at", "2026-01-02T12:00:00+01:00"),
            (2, "invalid_source_url", "file:///tmp/player.json"),
            (3, "invalid_license_url", "relative/license.txt"),
        ] {
            let (mut package, _) = fixture();
            let p = &mut provenance(&mut package)[0];
            match field {
                0 => p.as_of = Some(value.into()),
                1 => p.retrieved_at = Some(value.into()),
                2 => p.source_url = Some(value.into()),
                _ => p.license_url = Some(value.into()),
            }
            let reviews = vec![review(p)];
            fails(&package, &reviews, code);
        }
    }

    /// Given a versioned trusted registry, when decoded, then its camelCase contract is stable.
    #[test]
    fn trusted_review_registry_deserializes_versioned_json() {
        let (_, reviews) = fixture();
        let registry = TrustedRightsReviewRegistry {
            format_version: TRUSTED_RIGHTS_REVIEW_FORMAT_VERSION,
            reviews,
        };
        let json = serde_json::to_value(&registry).unwrap();
        assert_eq!(json["formatVersion"], TRUSTED_RIGHTS_REVIEW_FORMAT_VERSION);
        assert!(json["reviews"][0]["provenanceSha256"].is_string());
        let roundtrip: TrustedRightsReviewRegistry = serde_json::from_value(json).unwrap();
        assert_eq!(
            roundtrip.format_version,
            TRUSTED_RIGHTS_REVIEW_FORMAT_VERSION
        );
        assert_eq!(roundtrip.reviews.len(), 1);
    }

    /// Given unverified provenance, when approved, then data-kind verification is still required.
    #[test]
    fn unverified_data_fails() {
        let (mut package, _) = fixture();
        let p = &mut provenance(&mut package)[0];
        p.kind = RecordDataKind::Unverified;
        let reviews = vec![review(p)];
        fails(&package, &reviews, "unverified");
    }

    /// Given a blank manifest ID or no manifest, when checked, then no registry grants readiness.
    #[test]
    fn blank_ids_and_absent_manifest_fail() {
        let (mut package, reviews) = fixture();
        package.meta.as_mut().unwrap().id = " ".into();
        fails(&package, &reviews, "blank_id");
        package.meta = None;
        fails(&package, &[], "missing_manifest");
    }

    /// Given each serialized field changes, when hashed, then the previous approval becomes stale.
    #[test]
    fn fingerprint_covers_the_entire_provenance_record() {
        let p = record(RecordEntityKind::World, "sample");
        let fingerprint = provenance_fingerprint(&p).unwrap();
        let original = serde_json::to_value(&p).unwrap();
        for key in original.as_object().unwrap().keys() {
            let mut changed = original.clone();
            changed[key] = match key.as_str() {
                "entityKind" => serde_json::json!("country"),
                "kind" => serde_json::json!("estimated"),
                "distributionStatus" => serde_json::json!("restricted"),
                "providerIdentities" => {
                    serde_json::json!([{"provider":"other","providerEntityId":"other"}])
                }
                _ => serde_json::json!("changed"),
            };
            let changed: RecordProvenanceDef = serde_json::from_value(changed).unwrap();
            assert_ne!(
                fingerprint,
                provenance_fingerprint(&changed).unwrap(),
                "{key}"
            );
        }
        let roundtrip: RecordProvenanceDef = serde_json::from_value(original).unwrap();
        assert_eq!(fingerprint, provenance_fingerprint(&roundtrip).unwrap());
    }
    /// Given a reviewed player, when its payload changes, then the review becomes stale.
    #[test]
    fn player_payload_changes_invalidate_reviews() {
        let (mut package, mut reviews) = fixture();
        package.players.push(
            serde_json::from_value(serde_json::json!({"id":"p", "name":"Original"})).unwrap(),
        );
        let p = record(RecordEntityKind::Player, "p");
        let mut approval = review(&p);
        approval.record_sha256 =
            record_payload_fingerprint(&package, RecordEntityKind::Player, "p").unwrap();
        reviews.push(approval);
        provenance(&mut package).push(p);
        assert!(validate_package_distribution(&package, &reviews).is_empty());
        package.players[0].name = "Changed".into();
        fails(&package, &reviews, "stale_review");
    }

    /// Given reviewed metadata, when any non-provenance manifest field changes, then approval expires.
    #[test]
    fn manifest_changes_invalidate_reviews() {
        let (package, reviews) = fixture();
        let original = serde_json::to_value(package.meta.as_ref().unwrap()).unwrap();
        for key in original
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| *key != "recordProvenance" && *key != "id")
        {
            let (mut package, _) = fixture();
            let mut changed = original.clone();
            changed[key] = match key.as_str() {
                "formatVersion" => serde_json::json!(2),
                "baseYear" => serde_json::json!(2027),
                "defaultActiveRegions" | "defaultActiveCompetitions" => {
                    serde_json::json!(["changed"])
                }
                "fallbackLeague" => serde_json::json!({"name":"Changed"}),
                _ => serde_json::json!("Changed"),
            };
            package.meta = Some(serde_json::from_value(changed).unwrap());
            fails(&package, &reviews, "stale_review");
        }
    }

    /// Given reviewed name data, when its unaddressed definition metadata changes, then the world review expires.
    #[test]
    fn names_definition_metadata_invalidates_world_review() {
        let (mut package, mut reviews) = fixture();
        package.names = Some(
            serde_json::from_value(serde_json::json!({
                "version": 1,
                "description": "Original",
                "pools": {}
            }))
            .unwrap(),
        );
        reviews[0].record_sha256 =
            record_payload_fingerprint(&package, RecordEntityKind::World, "sample").unwrap();
        assert!(validate_package_distribution(&package, &reviews).is_empty());

        package.names.as_mut().unwrap().description = "Changed".into();

        fails(&package, &reviews, "stale_review");
    }

    /// Given a loaded translation bundle, when the typed distribution gate runs, then it fails closed.
    #[test]
    fn loaded_translations_are_not_distribution_records() {
        let (mut package, reviews) = fixture();
        package
            .extra_translations
            .insert("en".into(), serde_json::json!({"title": "unreviewed"}));

        fails(&package, &reviews, "unsupported_distribution_file");
    }

    /// Given equal typed pools with reordered maps, when hashed, then digests agree but array edits differ.
    #[test]
    fn name_pool_digest_is_deterministic() {
        let (mut package, _) = fixture();
        package.names = Some(serde_json::from_str(r#"{"pools":{"TR":{"last_names":["Can"],"first_names":["Ali","Veli"]},"GB":{"first_names":["John"],"last_names":["Smith"]}}}"#).unwrap());
        let first = record_payload_fingerprint(&package, RecordEntityKind::Names, "TR").unwrap();
        package.names = Some(serde_json::from_str(r#"{"pools":{"GB":{"last_names":["Smith"],"first_names":["John"]},"TR":{"first_names":["Ali","Veli"],"last_names":["Can"]}}}"#).unwrap());
        assert_eq!(
            first,
            record_payload_fingerprint(&package, RecordEntityKind::Names, "TR").unwrap()
        );
        package
            .names
            .as_mut()
            .unwrap()
            .pools
            .get_mut("TR")
            .unwrap()
            .first_names
            .reverse();
        assert_ne!(
            first,
            record_payload_fingerprint(&package, RecordEntityKind::Names, "TR").unwrap()
        );
        assert!(record_payload_fingerprint(&package, RecordEntityKind::Player, "absent").is_err());
    }

    /// Given an unaddressable entity kind, when a payload digest is requested, then hashing fails closed.
    #[test]
    fn unknown_entity_kind_cannot_be_fingerprinted() {
        let (package, _) = fixture();
        assert!(record_payload_fingerprint(&package, RecordEntityKind::Unknown, "sample").is_err());
    }

    /// Given ambiguous URL ports and unknown UTC offsets, when validated, then evidence is rejected.
    #[test]
    fn signed_ports_and_unknown_offsets_fail() {
        assert!(!evidence_url("https://example.org:+80/license"));
        assert!(!evidence_url("https://user@example.org/license"));
        assert!(!evidence_url("https://@example.org/license"));
        assert!(!evidence_url("https://example.org:/license"));
        assert!(evidence_url("https://[::1]:443/license"));
        assert!(!valid_retrieved_at("2026-01-02T12:00:00-00:00"));
        assert!(valid_retrieved_at("2026-01-02T12:00:00+00:00"));
    }

    fn inventory_fixture(
        files: &[(&str, &[u8])],
    ) -> (Vec<DistributionIssue>, Vec<DistributionIssue>) {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("ofm-distribution-{}", uuid::Uuid::new_v4()));
        let directory = root.join("data");
        std::fs::create_dir_all(&directory).unwrap();
        let archive_path = root.join("test.ofm");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        for (name, data) in files {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, data).unwrap();
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        let result = (
            validate_distribution_directory_files(&directory),
            validate_distribution_archive_files(&archive_path),
        );
        std::fs::remove_dir_all(&root).unwrap();
        result
    }

    /// Given only recognized JSON/YAML schemas, when either inventory is scanned, then every file passes.
    #[test]
    fn schema_data_files_pass_both_inventories() {
        let schemas = [
            "world",
            "team",
            "player",
            "staff",
            "confederation",
            "country",
            "competition",
            "names",
        ];
        let files: Vec<_> = schemas
            .iter()
            .enumerate()
            .map(|(i, schema)| {
                if i % 2 == 0 {
                    (
                        format!("data/{schema}.json"),
                        format!(r#"{{"schema":"{schema}"}}"#),
                    )
                } else {
                    (format!("data/{schema}.yaml"), format!("schema: {schema}"))
                }
            })
            .chain(std::iter::once((
                "data/uppercase.JSON".to_string(),
                r#"{"schema":"world"}"#.to_string(),
            )))
            .collect();
        let refs: Vec<_> = files
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_bytes()))
            .collect();
        let (directory, archive) = inventory_fixture(&refs);
        assert!(directory.is_empty(), "{directory:?}");
        assert!(archive.is_empty(), "{archive:?}");
    }

    /// Given sidecars, translations, assets and malformed schema files, when scanned, then both inventories reject them by path.
    #[test]
    fn unsupported_files_fail_both_inventories() {
        let files: &[(&str, &[u8])] = &[
            ("translations.tr.json", br#"{"schema":"world"}"#),
            ("Translations.en.json", br#"{"schema":"world"}"#),
            ("TRANSLATIONS.en.json", br#"{"schema":"world"}"#),
            ("assets/logo.png", b"png"),
            ("README.md", b"sidecar"),
            ("other.json", b"{}"),
            ("broken.yaml", b"schema: ["),
        ];
        let (directory, archive) = inventory_fixture(files);
        for (path, _) in files {
            assert!(directory.iter().any(|issue| issue.entity_id == *path));
            assert!(archive.iter().any(|issue| issue.entity_id == *path));
        }
        for path in [
            "translations.tr.json",
            "Translations.en.json",
            "TRANSLATIONS.en.json",
            "assets/logo.png",
            "README.md",
            "other.json",
        ] {
            assert!(directory.iter().any(|issue| {
                issue.entity_id == path && issue.code == "unsupported_distribution_file"
            }));
            assert!(archive.iter().any(|issue| {
                issue.entity_id == path && issue.code == "unsupported_distribution_file"
            }));
        }
    }

    /// Given an oversized schema and a missing root, when scanned, then no success is inferred.
    #[test]
    fn oversized_and_unreadable_inputs_fail() {
        let huge = vec![b' '; MAX_SCHEMA_BYTES as usize + 1];
        let (directory, archive) = inventory_fixture(&[("huge.json", &huge)]);
        assert_eq!(directory, archive);
        assert_eq!(directory[0].code, "oversized_distribution_file");
        let missing = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        assert!(!validate_distribution_directory_files(&missing).is_empty());
        assert!(!validate_distribution_archive_files(&missing).is_empty());
    }

    /// Given Unicode-normalized filename aliases, when either inventory is scanned, then both paths fail closed.
    #[test]
    fn non_ascii_distribution_paths_fail_in_both_inventories() {
        let composed = "data/caf\u{00e9}.json";
        let decomposed = "data/cafe\u{0301}.json";
        let files: &[(&str, &[u8])] = &[
            (composed, br#"{"schema":"world"}"#),
            (decomposed, br#"{"schema":"team"}"#),
        ];
        let (directory, archive) = inventory_fixture(files);
        assert_eq!(directory, archive);
        assert_eq!(
            directory
                .iter()
                .filter(|issue| issue.code == "unsafe_distribution_path")
                .count(),
            2,
            "distribution paths must be ASCII to avoid cross-filesystem normalization aliases"
        );
    }

    /// Given a ZIP directory entry carrying data, when inventoried, then its payload is rejected.
    #[test]
    fn nonempty_zip_directory_entries_fail_closed() {
        use std::io::Write;

        let path =
            std::env::temp_dir().join(format!("ofm-directory-entry-{}.ofm", uuid::Uuid::new_v4()));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("assets/logo.png/", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unreviewed directory payload").unwrap();
        zip.finish().unwrap();

        let assert_directory_payload_rejected = |issues: &[DistributionIssue]| {
            assert!(
                issues.iter().any(|issue| {
                    issue.code == "nonempty_distribution_directory_entry"
                        && issue.entity_id == "assets/logo.png/"
                }),
                "unexpected inventory issues: {issues:?}"
            );
        };
        assert_directory_payload_rejected(&validate_distribution_archive_files(&path));

        // Matching local and central headers can claim zero uncompressed bytes while retaining
        // the compressed stream. That hidden payload must also fail closed.
        let mut bytes = std::fs::read(&path).unwrap();
        let local_headers: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(i, signature)| (signature == b"PK\x03\x04").then_some(i))
            .collect();
        let central_headers: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(i, signature)| (signature == b"PK\x01\x02").then_some(i))
            .collect();
        assert_eq!(local_headers.len(), 1);
        assert_eq!(central_headers.len(), 1);
        bytes[local_headers[0] + 22..local_headers[0] + 26].fill(0);
        bytes[central_headers[0] + 24..central_headers[0] + 28].fill(0);
        std::fs::write(&path, bytes).unwrap();
        assert_directory_payload_rejected(&validate_distribution_archive_files(&path));

        std::fs::remove_file(path).unwrap();
    }

    /// Given low inventory limits, when directories or archives exceed them, then scanning stops closed.
    #[test]
    fn inventories_enforce_file_count_and_aggregate_size_limits() {
        use std::io::Write;

        let root =
            std::env::temp_dir().join(format!("ofm-distribution-limits-{}", uuid::Uuid::new_v4()));
        let directory = root.join("data");
        std::fs::create_dir_all(&directory).unwrap();
        let body = br#"{"schema":"world"}"#;
        std::fs::write(directory.join("one.json"), body).unwrap();
        std::fs::write(directory.join("two.json"), body).unwrap();

        let directory_count =
            validate_distribution_directory_files_with_limits(&directory, 1, u64::MAX);
        assert!(
            directory_count
                .iter()
                .any(|issue| issue.code == "too_many_distribution_files")
        );
        let directory_size = validate_distribution_directory_files_with_limits(&directory, 10, 1);
        assert!(
            directory_size
                .iter()
                .any(|issue| issue.code == "distribution_package_too_large")
        );

        let archive_path = root.join("data.ofm");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        for name in ["one.json", "two.json"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        let archive_count =
            validate_distribution_archive_files_with_limits(&archive_path, 1, u64::MAX, u64::MAX);
        assert!(
            archive_count
                .iter()
                .any(|issue| issue.code == "too_many_distribution_files")
        );
        let archive_size =
            validate_distribution_archive_files_with_limits(&archive_path, 10, 1, u64::MAX);
        assert!(
            archive_size
                .iter()
                .any(|issue| issue.code == "distribution_package_too_large")
        );
        let archive_bytes = std::fs::metadata(&archive_path).unwrap().len();
        let archive_file_size = validate_distribution_archive_files_with_limits(
            &archive_path,
            10,
            u64::MAX,
            archive_bytes.saturating_sub(1),
        );
        assert!(
            archive_file_size
                .iter()
                .any(|issue| issue.code == "oversized_distribution_archive")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Given traversal or duplicate ZIP names, when inventoried, then the offending path fails closed.
    #[test]
    fn unsafe_and_duplicate_archive_paths_fail() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("ofm-inventory-{}.ofm", uuid::Uuid::new_v4()));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for name in ["../escape.json", "a.json", "b.json"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(br#"{"schema":"world"}"#).unwrap();
        }
        zip.finish().unwrap();
        // ZipWriter rejects duplicate names, so mutate both local and central names without changing lengths.
        let mut bytes = std::fs::read(&path).unwrap();
        let offsets: Vec<_> = bytes
            .windows(6)
            .enumerate()
            .filter_map(|(i, part)| (part == b"b.json").then_some(i))
            .collect();
        assert_eq!(offsets.len(), 2);
        for offset in offsets {
            bytes[offset] = b'a';
        }
        std::fs::write(&path, bytes).unwrap();
        let issues = validate_distribution_archive_files(&path);
        assert!(
            issues
                .iter()
                .any(|issue| issue.entity_id == "../escape.json"
                    && issue.code == "unsafe_distribution_path")
        );
        assert!(issues.iter().any(|issue| issue.entity_id == "a.json"
            && issue.code == "invalid_distribution_archive_headers"));
        std::fs::remove_file(path).unwrap();
    }
}
