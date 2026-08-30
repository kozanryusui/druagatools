use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};

use serde::Deserialize;
use thiserror::Error;

/// Maximum accepted size of the authoritative catalog TOML control file.
pub const MAX_CATALOG_TOML_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub inspection_branch: InspectionBranchEvidence,
    pub artifact: Vec<Artifact>,
    #[serde(default)]
    pub unknown: Vec<Unknown>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionBranchEvidence {
    pub manifest_path: String,
    pub state: String,
    pub selected_branch: String,
    pub windows_xp_summary: String,
    pub evidence: Vec<String>,
    #[serde(default)]
    pub output: Vec<InspectionOutputEvidence>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionOutputEvidence {
    pub artifact_id: String,
    pub member: String,
    pub source_container: String,
    pub offset: u64,
    pub length: u64,
    pub output_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub source_path: String,
    pub original_filename: String,
    pub role: Role,
    pub stated_version: String,
    pub media_type: String,
    pub byte_size: u64,
    pub selection_status: SelectionStatus,
    pub derived_data_path: String,
    pub evidence_level: EvidenceLevel,
    pub notes: Vec<String>,
    #[serde(default)]
    pub observation: Vec<Observation>,
    #[serde(default)]
    pub hypothesis: Vec<Hypothesis>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub id: String,
    pub kind: String,
    pub value: String,
    pub evidence_level: EvidenceLevel,
    pub evidence: String,
    pub confidence: Confidence,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub evidence: String,
    pub confidence: Confidence,
    pub confirmation_test: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unknown {
    pub id: String,
    pub artifact_id: String,
    pub path: String,
    pub observed_properties: Vec<String>,
    pub evidence_level: EvidenceLevel,
    pub reason: String,
    pub possible_owner: String,
    pub destination_phase: u32,
    pub priority: Priority,
    pub blocking: bool,
    #[serde(default)]
    pub blocking_basis: Option<BlockingBasis>,
    pub confirmation_test: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Tower,
    Station,
    Dongle,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SelectionStatus {
    Selected,
    Primary,
    Comparison,
    Unselected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceLevel {
    Verified,
    Hypothesis,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Blocking,
    Required,
    Informational,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum BlockingBasis {
    SelectedTowerIdentity,
    SelectedStationIdentity,
    TowerEntryPoint,
    StatedVersions,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("catalog input/output failed for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "catalog input is larger than {maximum} bytes for {path}: observed at least {actual} bytes"
    )]
    CatalogTooLarge {
        path: String,
        maximum: u64,
        actual: u64,
    },
    #[error("catalog parse failed: {0}")]
    CatalogParse(#[from] toml::de::Error),
    #[error("catalog invariant failed: {0}")]
    CatalogInvariant(String),
    #[error("derived path is outside work/: {0}")]
    PathOutsideWork(String),
}

impl Catalog {
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != 1 {
            return Err(CatalogError::CatalogInvariant(
                "schema_version must be 1".to_owned(),
            ));
        }

        validate_inspection_branch(&self.inspection_branch)?;

        validate_authoritative_inventory(&self.artifact)?;

        let mut ids = HashSet::new();
        let mut observation_ids = HashSet::new();
        let mut hypothesis_ids = HashSet::new();
        for artifact in &self.artifact {
            if !ids.insert(artifact.id.as_str()) {
                return Err(CatalogError::CatalogInvariant(format!(
                    "duplicate artifact ID: {}",
                    artifact.id
                )));
            }
            validate_source_path(&artifact.source_path)?;
            validate_work_path(&artifact.derived_data_path)?;
            for observation in &artifact.observation {
                validate_observation(observation)?;
                if !observation_ids.insert(observation.id.as_str()) {
                    return Err(CatalogError::CatalogInvariant(format!(
                        "duplicate observation ID: {}",
                        observation.id
                    )));
                }
            }
            for hypothesis in &artifact.hypothesis {
                validate_hypothesis(hypothesis)?;
                if !hypothesis_ids.insert(hypothesis.id.as_str()) {
                    return Err(CatalogError::CatalogInvariant(format!(
                        "duplicate hypothesis ID: {}",
                        hypothesis.id
                    )));
                }
            }
        }

        let artifact_ids: HashSet<_> = self.artifact.iter().map(|item| item.id.as_str()).collect();
        for output in &self.inspection_branch.output {
            if !artifact_ids.contains(output.artifact_id.as_str()) {
                return Err(CatalogError::CatalogInvariant(format!(
                    "inspection output refers to missing artifact {}",
                    output.artifact_id
                )));
            }
        }
        let mut unknown_ids = HashSet::new();
        for unknown in &self.unknown {
            validate_unknown(unknown, &artifact_ids)?;
            if !unknown_ids.insert(unknown.id.as_str()) {
                return Err(CatalogError::CatalogInvariant(format!(
                    "duplicate unknown ID: {}",
                    unknown.id
                )));
            }
        }

        validate_selected_pair(&self.artifact)?;
        validate_selected_tower_evidence(&self.artifact)?;
        validate_dongles(&self.artifact)
    }

    pub fn tower_candidate(&self) -> Option<&Artifact> {
        self.artifact.iter().find(|artifact| {
            artifact.role == Role::Tower
                && artifact.selection_status == SelectionStatus::Selected
                && artifact.stated_version == "1.60"
        })
    }
}

#[derive(Clone, Copy)]
struct ExpectedArtifact {
    source_path: &'static str,
    role: Role,
    stated_version: &'static str,
    selection_status: SelectionStatus,
}

const EXPECTED_ARTIFACTS: [ExpectedArtifact; 12] = [
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL100-1-CT-MPRO-B [Ver.1.00] [Tower] (CD-ROM).iso",
        role: Role::Tower,
        stated_version: "1.00",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL110-1-CT-MPRO-C [Ver.1.10] [Tower] (CD-ROM).iso",
        role: Role::Tower,
        stated_version: "1.10",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL120-1-CT-MPRO-D [Ver.1.20] [Tower] (CD-ROM).iso",
        role: Role::Tower,
        stated_version: "1.20",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL160-1-CT-MPRO-H [Ver.1.60] [Tower] (CD-ROM).iso",
        role: Role::Tower,
        stated_version: "1.60",
        selection_status: SelectionStatus::Selected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL1001-ST-DVD0-A [Ver.1.00] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.00",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL110-1-ST-DVD0-C [Ver.1.10] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.10",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL120-1-ST-DVD0-D [Ver.1.20] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.20",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL140-1-ST-DVD0-F [Ver.1.40] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.40",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL150-1-ST-DVD0-G [Ver.1.50] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.50",
        selection_status: SelectionStatus::Unselected,
    },
    ExpectedArtifact {
        source_path: "iso/NM00028 DOL160-1-ST-DVD0-H [Ver.1.60] [Station] (DVD-ROM).iso",
        role: Role::Station,
        stated_version: "1.60",
        selection_status: SelectionStatus::Selected,
    },
    ExpectedArtifact {
        source_path: "dongle/NM00028 DOL1401-ST-F, Ver.F a026241356508a [Station] [Rebuilt].bin",
        role: Role::Dongle,
        stated_version: "1.40",
        selection_status: SelectionStatus::Comparison,
    },
    ExpectedArtifact {
        source_path: "dongle/NM00028 DOL165-1-ST-I, Ver.I a026241387685a [Station].bin",
        role: Role::Dongle,
        stated_version: "1.65",
        selection_status: SelectionStatus::Primary,
    },
];

fn validate_authoritative_inventory(artifacts: &[Artifact]) -> Result<(), CatalogError> {
    if artifacts.len() != EXPECTED_ARTIFACTS.len() {
        return Err(CatalogError::CatalogInvariant(format!(
            "catalog must contain exactly {} artifacts",
            EXPECTED_ARTIFACTS.len()
        )));
    }

    let mut source_paths = HashSet::new();
    for artifact in artifacts {
        validate_required("artifact ID", &artifact.id)?;
        validate_required("artifact original filename", &artifact.original_filename)?;
        validate_required("artifact stated version", &artifact.stated_version)?;
        validate_required("artifact media type", &artifact.media_type)?;
        validate_required("artifact derived-data path", &artifact.derived_data_path)?;
        if artifact.byte_size == 0 {
            return Err(CatalogError::CatalogInvariant(format!(
                "artifact {} must have a nonzero byte size",
                artifact.id
            )));
        }
        if artifact.notes.is_empty() || artifact.notes.iter().any(|note| note.trim().is_empty()) {
            return Err(CatalogError::CatalogInvariant(format!(
                "artifact {} must have nonblank notes",
                artifact.id
            )));
        }
        if artifact.evidence_level != EvidenceLevel::Verified {
            return Err(CatalogError::CatalogInvariant(format!(
                "artifact {} must use verified evidence",
                artifact.id
            )));
        }
        if !source_paths.insert(artifact.source_path.as_str()) {
            return Err(CatalogError::CatalogInvariant(format!(
                "duplicate artifact source path: {}",
                artifact.source_path
            )));
        }
        let filename = Path::new(&artifact.source_path)
            .file_name()
            .and_then(|value| value.to_str());
        if filename != Some(artifact.original_filename.as_str()) {
            return Err(CatalogError::CatalogInvariant(format!(
                "artifact {} filename does not match its source path",
                artifact.id
            )));
        }
        let expected = EXPECTED_ARTIFACTS
            .iter()
            .find(|expected| expected.source_path == artifact.source_path)
            .ok_or_else(|| {
                CatalogError::CatalogInvariant(format!(
                    "unexpected artifact source path: {}",
                    artifact.source_path
                ))
            })?;
        if artifact.role != expected.role
            || artifact.stated_version != expected.stated_version
            || artifact.selection_status != expected.selection_status
        {
            return Err(CatalogError::CatalogInvariant(format!(
                "artifact {} has an invalid role, version, or selection status",
                artifact.id
            )));
        }
    }

    for expected in EXPECTED_ARTIFACTS {
        if !source_paths.contains(expected.source_path) {
            return Err(CatalogError::CatalogInvariant(format!(
                "missing artifact source path: {}",
                expected.source_path
            )));
        }
    }
    Ok(())
}

fn validate_inspection_branch(branch: &InspectionBranchEvidence) -> Result<(), CatalogError> {
    validate_required("inspection manifest path", &branch.manifest_path)?;
    if branch.manifest_path
        != ".planning/phases/01-artifact-and-version-baseline/01-INSPECTION-BRANCH.toml"
    {
        return Err(CatalogError::CatalogInvariant(
            "inspection branch must cite the Phase 1 manifest".to_owned(),
        ));
    }
    if branch.state != "complete" {
        return Err(CatalogError::CatalogInvariant(
            "inspection branch state must be complete".to_owned(),
        ));
    }
    if branch.selected_branch != "bounded-container" {
        return Err(CatalogError::CatalogInvariant(
            "the completed inspection branch must be bounded-container".to_owned(),
        ));
    }
    if branch.windows_xp_summary != "not-applicable" {
        return Err(CatalogError::CatalogInvariant(
            "Windows XP summary must be recorded as not-applicable".to_owned(),
        ));
    }
    if branch.evidence.is_empty() || branch.evidence.iter().any(|item| item.trim().is_empty()) {
        return Err(CatalogError::CatalogInvariant(
            "inspection branch must include evidence".to_owned(),
        ));
    }

    let expected = [
        ("tower-1.60", "work/tower/1.60/conditional/member-01.bin"),
        (
            "station-1.60",
            "work/station/1.60/conditional/member-01.bin",
        ),
    ];
    if branch.output.len() != expected.len() {
        return Err(CatalogError::CatalogInvariant(
            "bounded-container evidence must list two outputs".to_owned(),
        ));
    }
    for (artifact_id, output_path) in expected {
        let output = branch
            .output
            .iter()
            .find(|item| item.artifact_id == artifact_id)
            .ok_or_else(|| {
                CatalogError::CatalogInvariant(format!("missing bounded output for {artifact_id}"))
            })?;
        if output.member != "p40a00.vp2"
            || output.source_container != "/GAME.DAT"
            || output.offset != 1216
            || output.length != 109600
            || output.output_path != output_path
        {
            return Err(CatalogError::CatalogInvariant(format!(
                "bounded output for {artifact_id} does not match the completed manifest"
            )));
        }
        validate_work_path(&output.output_path)?;
    }
    Ok(())
}

impl Artifact {
    pub fn observation_by_kind(&self, kind: &str) -> Option<&Observation> {
        self.observation
            .iter()
            .find(|observation| observation.kind == kind)
    }

    pub fn hypothesis_by_id(&self, id: &str) -> Option<&Hypothesis> {
        self.hypothesis
            .iter()
            .find(|hypothesis| hypothesis.id == id)
    }
}

fn validate_observation(observation: &Observation) -> Result<(), CatalogError> {
    validate_required("observation ID", &observation.id)?;
    validate_required("observation kind", &observation.kind)?;
    validate_required("observation value", &observation.value)?;
    validate_required("observation evidence", &observation.evidence)?;
    if observation.evidence_level != EvidenceLevel::Verified {
        return Err(CatalogError::CatalogInvariant(format!(
            "observation {} must use verified evidence",
            observation.id
        )));
    }
    Ok(())
}

fn validate_hypothesis(hypothesis: &Hypothesis) -> Result<(), CatalogError> {
    validate_required("hypothesis ID", &hypothesis.id)?;
    validate_required("hypothesis statement", &hypothesis.statement)?;
    validate_required("hypothesis evidence", &hypothesis.evidence)?;
    validate_required(
        "hypothesis confirmation test",
        &hypothesis.confirmation_test,
    )
}

fn validate_unknown(unknown: &Unknown, artifact_ids: &HashSet<&str>) -> Result<(), CatalogError> {
    validate_required("unknown ID", &unknown.id)?;
    validate_required("unknown artifact ID", &unknown.artifact_id)?;
    validate_required("unknown path", &unknown.path)?;
    validate_required("unknown reason", &unknown.reason)?;
    validate_required("unknown possible owner", &unknown.possible_owner)?;
    validate_required("unknown confirmation test", &unknown.confirmation_test)?;
    if unknown.observed_properties.is_empty()
        || unknown
            .observed_properties
            .iter()
            .any(|item| item.trim().is_empty())
    {
        return Err(CatalogError::CatalogInvariant(format!(
            "unknown {} must have observed properties",
            unknown.id
        )));
    }
    if unknown.evidence_level != EvidenceLevel::Unknown {
        return Err(CatalogError::CatalogInvariant(format!(
            "unknown {} must use unknown evidence",
            unknown.id
        )));
    }
    if !artifact_ids.contains(unknown.artifact_id.as_str()) {
        return Err(CatalogError::CatalogInvariant(format!(
            "unknown {} refers to missing artifact {}",
            unknown.id, unknown.artifact_id
        )));
    }
    if unknown.destination_phase == 0 {
        return Err(CatalogError::CatalogInvariant(format!(
            "unknown {} must have a destination phase",
            unknown.id
        )));
    }
    match (unknown.blocking, unknown.priority, unknown.blocking_basis) {
        (true, Priority::Blocking, Some(_))
        | (false, Priority::Required, None)
        | (false, Priority::Informational, None) => Ok(()),
        _ => Err(CatalogError::CatalogInvariant(format!(
            "unknown {} has an invalid Phase 1 blocking status",
            unknown.id
        ))),
    }
}

fn validate_required(field: &str, value: &str) -> Result<(), CatalogError> {
    if value.trim().is_empty() {
        return Err(CatalogError::CatalogInvariant(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

pub fn load_catalog(path: &Path) -> Result<Catalog, CatalogError> {
    let display_path = path.display().to_string();
    let metadata = fs::metadata(path).map_err(|source| CatalogError::Io {
        path: display_path.clone(),
        source,
    })?;
    if metadata.len() > MAX_CATALOG_TOML_BYTES {
        return Err(CatalogError::CatalogTooLarge {
            path: display_path,
            maximum: MAX_CATALOG_TOML_BYTES,
            actual: metadata.len(),
        });
    }

    let file = File::open(path).map_err(|source| CatalogError::Io {
        path: display_path.clone(),
        source,
    })?;
    let mut text = String::with_capacity(MAX_CATALOG_TOML_BYTES as usize);
    file.take(MAX_CATALOG_TOML_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|source| CatalogError::Io {
            path: display_path.clone(),
            source,
        })?;
    if text.len() as u64 > MAX_CATALOG_TOML_BYTES {
        return Err(CatalogError::CatalogTooLarge {
            path: display_path,
            maximum: MAX_CATALOG_TOML_BYTES,
            actual: text.len() as u64,
        });
    }

    load_catalog_from_str(&text)
}

pub fn load_catalog_from_str(text: &str) -> Result<Catalog, CatalogError> {
    let catalog: Catalog = toml::from_str(text)?;
    catalog.validate()?;
    Ok(catalog)
}

fn validate_source_path(path: &str) -> Result<(), CatalogError> {
    let path = Path::new(path);
    let first = path.components().next();
    if path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
        || !matches!(first, Some(Component::Normal(value)) if value == "iso" || value == "dongle")
    {
        return Err(CatalogError::CatalogInvariant(format!(
            "source path must be under iso/ or dongle/: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_work_path(path: &str) -> Result<(), CatalogError> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
        || !matches!(path.components().next(), Some(Component::Normal(value)) if value == "work")
    {
        return Err(CatalogError::PathOutsideWork(path.display().to_string()));
    }
    Ok(())
}

fn validate_selected_pair(artifacts: &[Artifact]) -> Result<(), CatalogError> {
    for role in [Role::Tower, Role::Station] {
        let selected: Vec<_> = artifacts
            .iter()
            .filter(|item| item.role == role && item.selection_status == SelectionStatus::Selected)
            .collect();
        if selected.len() != 1 || selected[0].stated_version != "1.60" {
            return Err(CatalogError::CatalogInvariant(format!(
                "exactly one {role:?} version 1.60 must be selected"
            )));
        }
    }
    Ok(())
}

fn validate_selected_tower_evidence(artifacts: &[Artifact]) -> Result<(), CatalogError> {
    const SELECTED_TOWER_PATH: &str =
        "iso/NM00028 DOL160-1-CT-MPRO-H [Ver.1.60] [Tower] (CD-ROM).iso";
    const REQUIRED_OBSERVATION_KINDS: [&str; 7] = [
        "file-structure",
        "pe-file-format",
        "pe-sections",
        "pe-imports",
        "pe-exports-resources",
        "visible-version-marker",
        "selected-pair",
    ];

    let tower = artifacts
        .iter()
        .find(|artifact| artifact.source_path == SELECTED_TOWER_PATH)
        .ok_or_else(|| {
            CatalogError::CatalogInvariant("selected Tower artifact is missing".to_owned())
        })?;

    for kind in REQUIRED_OBSERVATION_KINDS {
        let count = tower
            .observation
            .iter()
            .filter(|observation| observation.kind == kind)
            .count();
        if count != 1 {
            return Err(CatalogError::CatalogInvariant(format!(
                "selected Tower must contain exactly one {kind} observation"
            )));
        }
    }

    let hypotheses: Vec<_> = tower
        .hypothesis
        .iter()
        .filter(|hypothesis| hypothesis.id == "HYP-TOWER-001")
        .collect();
    if hypotheses.len() != 1 {
        return Err(CatalogError::CatalogInvariant(
            "selected Tower must contain HYP-TOWER-001 exactly once".to_owned(),
        ));
    }
    if hypotheses[0].confidence != Confidence::Medium {
        return Err(CatalogError::CatalogInvariant(
            "HYP-TOWER-001 must use medium confidence".to_owned(),
        ));
    }
    Ok(())
}

fn validate_dongles(artifacts: &[Artifact]) -> Result<(), CatalogError> {
    let has_primary = artifacts.iter().any(|item| {
        item.role == Role::Dongle
            && item.stated_version == "1.65"
            && item.selection_status == SelectionStatus::Primary
    });
    let has_comparison = artifacts.iter().any(|item| {
        item.role == Role::Dongle
            && item.stated_version == "1.40"
            && item.selection_status == SelectionStatus::Comparison
    });
    if !has_primary || !has_comparison {
        return Err(CatalogError::CatalogInvariant(
            "dongle 1.65 must be primary and dongle 1.40 must be comparison data".to_owned(),
        ));
    }
    Ok(())
}
