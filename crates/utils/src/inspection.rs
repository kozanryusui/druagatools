use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::atomic_output::{AtomicOutputError, replace_all, stage_file, write_bytes};

const MAX_MEMBER_LENGTH: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_LENGTH: u64 = 64 * 1024 * 1024;
/// Maximum accepted size of an inspection manifest TOML control file.
pub const MAX_INSPECTION_TOML_BYTES: u64 = 16 * 1024;
const TOWER_SOURCE: &str = "iso/NM00028 DOL160-1-CT-MPRO-H [Ver.1.60] [Tower] (CD-ROM).iso";
const STATION_SOURCE: &str = "iso/NM00028 DOL160-1-ST-DVD0-H [Ver.1.60] [Station] (DVD-ROM).iso";
const XP_OUTPUT: &str = "work/tower/1.60/xp-summary.md";
const CONDITIONAL_OUTPUTS: [&str; 4] = [
    "work/tower/1.60/conditional/member-01.bin",
    "work/tower/1.60/conditional/member-02.bin",
    "work/station/1.60/conditional/member-01.bin",
    "work/station/1.60/conditional/member-02.bin",
];

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionManifest {
    pub schema_version: u32,
    pub state: InspectionState,
    pub branch: InspectionBranch,
    pub combined_output_limit: u64,
    #[serde(default)]
    pub evidence_range: Vec<EvidenceRange>,
    #[serde(default)]
    pub member: Vec<Member>,
    #[serde(default)]
    pub xp_summary_output: Option<String>,
    #[serde(default)]
    pub standard_area: Vec<StandardArea>,
    #[serde(default)]
    pub detail: Vec<Detail>,
    #[serde(default)]
    pub unknown: Vec<Unknown>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionState {
    Approved,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionBranch {
    BoundedContainer,
    WindowsXpSummary,
    NotApplicable,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRange {
    pub id: String,
    pub source_path: String,
    pub container_path: String,
    pub offset: u64,
    pub length: u64,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub source_path: String,
    pub container_path: String,
    pub internal_member_path: String,
    pub offset: u64,
    pub length: u64,
    pub output_path: String,
    pub evidence_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandardArea {
    pub name: String,
    pub purpose: String,
    pub size: u64,
    pub file_count: u64,
    pub evidence_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Detail {
    pub category: DetailCategory,
    pub name: String,
    pub description: String,
    pub evidence_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetailCategory {
    GameFiles,
    CustomServices,
    Drivers,
    Configuration,
    NonstandardDependencies,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unknown {
    pub id: String,
    pub path: String,
    pub observed_properties: Vec<String>,
    pub evidence: String,
    pub confirmation_test: String,
    pub possible_owner: String,
    pub destination_phase: u32,
    pub priority: UnknownPriority,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownPriority {
    Blocking,
    Required,
    Informational,
}

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error(transparent)]
    AtomicOutput(#[from] AtomicOutputError),
    #[error("inspection input/output failed for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "inspection manifest input is larger than {maximum} bytes for {path}: observed at least {actual} bytes"
    )]
    ManifestTooLarge {
        path: String,
        maximum: u64,
        actual: u64,
    },
    #[error("inspection manifest parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("inspection manifest serialization failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("inspection manifest invariant failed: {0}")]
    Invariant(String),
    #[error("bounded extraction failed for {output}: {status}")]
    Extraction { output: String, status: String },
}

impl InspectionManifest {
    pub fn validate(&self) -> Result<(), InspectionError> {
        if self.schema_version != 1 {
            return invariant("schema_version must be 1");
        }

        let evidence_ids = self.validate_evidence()?;
        match self.branch {
            InspectionBranch::BoundedContainer => self.validate_bounded(&evidence_ids),
            InspectionBranch::WindowsXpSummary => self.validate_xp(&evidence_ids),
            InspectionBranch::NotApplicable => self.validate_not_applicable(),
        }
    }

    fn validate_evidence(&self) -> Result<HashSet<&str>, InspectionError> {
        let mut ids = HashSet::new();
        for range in &self.evidence_range {
            required("evidence ID", &range.id)?;
            required("evidence description", &range.description)?;
            validate_source(&range.source_path)?;
            validate_container(&range.container_path)?;
            validate_range(range.offset, range.length, "evidence range")?;
            if !ids.insert(range.id.as_str()) {
                return invariant(format!("duplicate evidence ID: {}", range.id));
            }
        }
        Ok(ids)
    }

    fn validate_bounded(&self, evidence_ids: &HashSet<&str>) -> Result<(), InspectionError> {
        if self.member.is_empty() || self.member.len() > 4 {
            return invariant("bounded-container requires one to four members");
        }
        if self.xp_summary_output.is_some()
            || !self.standard_area.is_empty()
            || !self.detail.is_empty()
            || !self.unknown.is_empty()
        {
            return invariant("bounded-container contains records for a different branch");
        }
        if self.combined_output_limit == 0 || self.combined_output_limit > MAX_TOTAL_LENGTH {
            return invariant("combined output limit must be from 1 byte through 64 MiB");
        }

        let mut total = 0_u64;
        let mut outputs = HashSet::new();
        for member in &self.member {
            validate_source(&member.source_path)?;
            validate_container(&member.container_path)?;
            required("internal member path", &member.internal_member_path)?;
            if Path::new(&member.internal_member_path).is_absolute()
                || Path::new(&member.internal_member_path)
                    .components()
                    .any(|part| part == Component::ParentDir)
            {
                return invariant("internal member path must be a relative member name");
            }
            validate_range(member.offset, member.length, "member")?;
            validate_member_output(&member.source_path, &member.output_path)?;
            if !outputs.insert(member.output_path.as_str()) {
                return invariant(format!("duplicate output path: {}", member.output_path));
            }
            if !evidence_ids.contains(member.evidence_id.as_str()) {
                return invariant(format!("missing evidence ID: {}", member.evidence_id));
            }
            total = total
                .checked_add(member.length)
                .ok_or_else(|| InspectionError::Invariant("member total overflow".to_owned()))?;
        }
        if total > self.combined_output_limit || total > MAX_TOTAL_LENGTH {
            return invariant("member total exceeds the approved output limit");
        }
        Ok(())
    }

    fn validate_xp(&self, evidence_ids: &HashSet<&str>) -> Result<(), InspectionError> {
        if !self.member.is_empty() || !self.unknown.is_empty() || self.combined_output_limit != 0 {
            return invariant("windows-xp-summary contains records for a different branch");
        }
        if self.xp_summary_output.as_deref() != Some(XP_OUTPUT) {
            return invariant("Windows XP summary output is not the fixed output path");
        }
        if self.standard_area.is_empty() {
            return invariant("Windows XP summary requires standard system areas");
        }
        for area in &self.standard_area {
            required("standard area name", &area.name)?;
            required("standard area purpose", &area.purpose)?;
            validate_evidence_reference(evidence_ids, &area.evidence_id)?;
        }
        let categories: HashSet<_> = self.detail.iter().map(|item| item.category).collect();
        let required_categories = [
            DetailCategory::GameFiles,
            DetailCategory::CustomServices,
            DetailCategory::Drivers,
            DetailCategory::Configuration,
            DetailCategory::NonstandardDependencies,
        ];
        if !required_categories
            .iter()
            .all(|category| categories.contains(category))
        {
            return invariant("Windows XP summary is missing one or more detailed categories");
        }
        for detail in &self.detail {
            required("detail name", &detail.name)?;
            required("detail description", &detail.description)?;
            validate_evidence_reference(evidence_ids, &detail.evidence_id)?;
        }
        Ok(())
    }

    fn validate_not_applicable(&self) -> Result<(), InspectionError> {
        if !self.member.is_empty()
            || self.xp_summary_output.is_some()
            || !self.standard_area.is_empty()
            || !self.detail.is_empty()
            || self.combined_output_limit != 0
        {
            return invariant("not-applicable contains outputs for a different branch");
        }
        if self.unknown.is_empty() {
            return invariant("not-applicable requires at least one stable unknown record");
        }
        let mut ids = HashSet::new();
        for unknown in &self.unknown {
            required("unknown ID", &unknown.id)?;
            required("unknown path", &unknown.path)?;
            required("unknown evidence", &unknown.evidence)?;
            required("unknown confirmation test", &unknown.confirmation_test)?;
            required("unknown possible owner", &unknown.possible_owner)?;
            if unknown.destination_phase == 0
                || unknown.observed_properties.is_empty()
                || unknown
                    .observed_properties
                    .iter()
                    .any(|value| value.trim().is_empty())
            {
                return invariant("unknown record is incomplete");
            }
            if !ids.insert(unknown.id.as_str()) {
                return invariant(format!("duplicate unknown ID: {}", unknown.id));
            }
        }
        Ok(())
    }
}

pub fn load_inspection(path: &Path) -> Result<InspectionManifest, InspectionError> {
    let display_path = path.display().to_string();
    let metadata = fs::metadata(path).map_err(|source| InspectionError::Io {
        path: display_path.clone(),
        source,
    })?;
    if metadata.len() > MAX_INSPECTION_TOML_BYTES {
        return Err(InspectionError::ManifestTooLarge {
            path: display_path,
            maximum: MAX_INSPECTION_TOML_BYTES,
            actual: metadata.len(),
        });
    }

    let file = File::open(path).map_err(|source| InspectionError::Io {
        path: display_path.clone(),
        source,
    })?;
    let mut text = String::with_capacity(MAX_INSPECTION_TOML_BYTES as usize);
    file.take(MAX_INSPECTION_TOML_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|source| InspectionError::Io {
            path: display_path.clone(),
            source,
        })?;
    if text.len() as u64 > MAX_INSPECTION_TOML_BYTES {
        return Err(InspectionError::ManifestTooLarge {
            path: display_path,
            maximum: MAX_INSPECTION_TOML_BYTES,
            actual: text.len() as u64,
        });
    }

    load_inspection_from_str(&text)
}

pub fn load_inspection_from_str(text: &str) -> Result<InspectionManifest, InspectionError> {
    let manifest: InspectionManifest = toml::from_str(text)?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn execute_inspection_branch(path: &Path) -> Result<(), InspectionError> {
    let manifest = load_inspection(path)?;
    if manifest.state != InspectionState::Approved {
        return invariant("execution requires manifest state approved");
    }
    match manifest.branch {
        InspectionBranch::BoundedContainer => execute_bounded(&manifest.member),
        InspectionBranch::WindowsXpSummary => write_xp_summary(&manifest),
        InspectionBranch::NotApplicable => validate_conditional_outputs_absent(),
    }
}

pub fn finalize_inspection_branch(path: &Path) -> Result<(), InspectionError> {
    let mut manifest = load_inspection(path)?;
    if manifest.state != InspectionState::Approved {
        return invariant("finalization requires manifest state approved");
    }
    let result = validate_final_outputs(&manifest);
    manifest.state = if result.is_ok() {
        InspectionState::Complete
    } else {
        InspectionState::Failed
    };
    write_manifest(path, &manifest)?;
    result
}

pub fn require_complete(path: &Path) -> Result<(), InspectionError> {
    let manifest = load_inspection(path)?;
    if manifest.state != InspectionState::Complete {
        return invariant("manifest state must be complete");
    }
    validate_final_outputs(&manifest)
}

fn execute_bounded(members: &[Member]) -> Result<(), InspectionError> {
    let mut staged = Vec::with_capacity(members.len());
    for member in members {
        let output = Path::new(&member.output_path);
        let parent = output
            .parent()
            .ok_or_else(|| InspectionError::Invariant("output has no parent".to_owned()))?;
        fs::create_dir_all(parent).map_err(|source| InspectionError::Io {
            path: parent.display().to_string(),
            source,
        })?;
        let staged_output = stage_file(output, member.length, |staged_path| {
            let status = Command::new("xorriso")
                .args([
                    "-osirrox",
                    "on",
                    "-indev",
                    &member.source_path,
                    "-extract_cut",
                    &member.container_path,
                    &member.offset.to_string(),
                    &member.length.to_string(),
                ])
                .arg(staged_path)
                .status()
                .map_err(|source| AtomicOutputError::Io {
                    path: "xorriso".to_owned(),
                    source,
                })?;
            if !status.success() {
                return Err(AtomicOutputError::Operation(format!(
                    "bounded extraction failed for {}: {status}",
                    member.output_path
                )));
            }
            Ok(())
        })?;
        staged.push(staged_output);
    }
    replace_all(staged)?;
    Ok(())
}

fn write_xp_summary(manifest: &InspectionManifest) -> Result<(), InspectionError> {
    let mut text = String::from("# Windows XP Metadata Summary\n\n## Standard system areas\n\n");
    for area in &manifest.standard_area {
        text.push_str(&format!(
            "- {}: {}; size={}; file_count={}; evidence={}\n",
            area.name, area.purpose, area.size, area.file_count, area.evidence_id
        ));
    }
    text.push_str("\n## Detailed records\n\n");
    for detail in &manifest.detail {
        text.push_str(&format!(
            "- {:?}: {} — {}; evidence={}\n",
            detail.category, detail.name, detail.description, detail.evidence_id
        ));
    }
    let output = manifest
        .xp_summary_output
        .as_deref()
        .ok_or_else(|| InspectionError::Invariant("XP output is absent".to_owned()))?;
    if let Some(parent) = Path::new(output).parent() {
        fs::create_dir_all(parent).map_err(|source| InspectionError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    write_bytes(Path::new(output), text.as_bytes())?;
    Ok(())
}

fn validate_final_outputs(manifest: &InspectionManifest) -> Result<(), InspectionError> {
    match manifest.branch {
        InspectionBranch::BoundedContainer => {
            let mut total = 0_u64;
            for member in &manifest.member {
                let size = fs::metadata(&member.output_path)
                    .map_err(|source| InspectionError::Io {
                        path: member.output_path.clone(),
                        source,
                    })?
                    .len();
                if size != member.length {
                    return invariant(format!(
                        "output {} has length {size}, expected {}",
                        member.output_path, member.length
                    ));
                }
                total = total.checked_add(size).ok_or_else(|| {
                    InspectionError::Invariant("output total overflow".to_owned())
                })?;
            }
            if total > manifest.combined_output_limit || total > MAX_TOTAL_LENGTH {
                return invariant("final output total exceeds the approved limit");
            }
            Ok(())
        }
        InspectionBranch::WindowsXpSummary => {
            let output = manifest.xp_summary_output.as_deref().unwrap_or_default();
            if fs::metadata(output).map(|item| item.len()).unwrap_or(0) == 0 {
                return invariant("Windows XP metadata summary is absent or empty");
            }
            Ok(())
        }
        InspectionBranch::NotApplicable => validate_conditional_outputs_absent(),
    }
}

fn validate_conditional_outputs_absent() -> Result<(), InspectionError> {
    for output in CONDITIONAL_OUTPUTS.into_iter().chain([XP_OUTPUT]) {
        if Path::new(output).exists() {
            return invariant(format!("not-applicable output exists: {output}"));
        }
    }
    Ok(())
}

fn write_manifest(path: &Path, manifest: &InspectionManifest) -> Result<(), InspectionError> {
    let text = toml::to_string_pretty(manifest)?;
    write_bytes(path, text.as_bytes())?;
    Ok(())
}

fn validate_source(source: &str) -> Result<(), InspectionError> {
    required("source path", source)?;
    if source != TOWER_SOURCE && source != STATION_SOURCE {
        return invariant(format!(
            "source path is not an approved selected ISO: {source}"
        ));
    }
    Ok(())
}

fn validate_container(container: &str) -> Result<(), InspectionError> {
    if container != "/GAME.DAT" && container != "/INFO.DAT" {
        return invariant(format!("container path is not approved: {container}"));
    }
    Ok(())
}

fn validate_member_output(source: &str, output: &str) -> Result<(), InspectionError> {
    validate_relative_work_path(output)?;
    let permitted = match source {
        TOWER_SOURCE => &CONDITIONAL_OUTPUTS[..2],
        STATION_SOURCE => &CONDITIONAL_OUTPUTS[2..],
        _ => return invariant("member source is not approved"),
    };
    if !permitted.contains(&output) {
        return invariant(format!(
            "member output is not an approved fixed slot: {output}"
        ));
    }
    Ok(())
}

fn validate_relative_work_path(value: &str) -> Result<(), InspectionError> {
    required("output path", value)?;
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
        || !matches!(path.components().next(), Some(Component::Normal(first)) if first == "work")
    {
        return invariant(format!("output path is outside work/: {value}"));
    }
    Ok(())
}

fn validate_range(offset: u64, length: u64, name: &str) -> Result<(), InspectionError> {
    if length == 0 || length > MAX_MEMBER_LENGTH {
        return invariant(format!("{name} length must be from 1 byte through 16 MiB"));
    }
    offset
        .checked_add(length)
        .ok_or_else(|| InspectionError::Invariant(format!("{name} offset and length overflow")))?;
    Ok(())
}

fn validate_evidence_reference(
    evidence_ids: &HashSet<&str>,
    evidence_id: &str,
) -> Result<(), InspectionError> {
    required("evidence reference", evidence_id)?;
    if !evidence_ids.contains(evidence_id) {
        return invariant(format!("missing evidence ID: {evidence_id}"));
    }
    Ok(())
}

fn required(name: &str, value: &str) -> Result<(), InspectionError> {
    if value.trim().is_empty() {
        return invariant(format!("{name} must not be blank"));
    }
    Ok(())
}

fn invariant<T>(message: impl Into<String>) -> Result<T, InspectionError> {
    Err(InspectionError::Invariant(message.into()))
}
