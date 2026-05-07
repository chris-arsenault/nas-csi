//! CSI controller-side policy validation.

use nas_csi_types::AccessMode as PolicyAccessMode;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VolumeMode {
    Filesystem,
    Block,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CsiAccessMode {
    SingleNodeWriter,
    SingleNodeReaderOnly,
    MultiNodeReaderOnly,
    MultiNodeSingleWriter,
    MultiNodeMultiWriter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumePolicy {
    pub name: String,
    pub access: PolicyAccessMode,
    pub allow_multi_node_writer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRequest {
    pub volume_mode: VolumeMode,
    pub access_mode: CsiAccessMode,
    pub policy: VolumePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityValidation {
    Valid { warnings: Vec<String> },
    Invalid { reason: String },
}

pub fn validate_capability(request: &CapabilityRequest) -> CapabilityValidation {
    if request.volume_mode == VolumeMode::Block {
        return CapabilityValidation::Invalid {
            reason: "block volume mode is not supported for same-dataset storage".to_string(),
        };
    }

    match request.policy.access {
        PolicyAccessMode::ReadOnly => validate_read_only_policy(request),
        PolicyAccessMode::ReadWrite => validate_read_write_policy(request),
    }
}

fn validate_read_only_policy(request: &CapabilityRequest) -> CapabilityValidation {
    match request.access_mode {
        CsiAccessMode::SingleNodeReaderOnly | CsiAccessMode::MultiNodeReaderOnly => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
        _ => CapabilityValidation::Invalid {
            reason: format!("policy {} is read-only", request.policy.name),
        },
    }
}

fn validate_read_write_policy(request: &CapabilityRequest) -> CapabilityValidation {
    match request.access_mode {
        CsiAccessMode::SingleNodeWriter | CsiAccessMode::MultiNodeSingleWriter => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
        CsiAccessMode::MultiNodeMultiWriter if request.policy.allow_multi_node_writer => {
            CapabilityValidation::Valid {
                warnings: vec![
                    "multi-node writer uses shared filesystem semantics; application-level write conflicts remain possible".to_string(),
                ],
            }
        }
        CsiAccessMode::MultiNodeMultiWriter => CapabilityValidation::Invalid {
            reason: format!(
                "policy {} does not allow multi-node writer",
                request.policy.name
            ),
        },
        CsiAccessMode::SingleNodeReaderOnly | CsiAccessMode::MultiNodeReaderOnly => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_write_policy() -> VolumePolicy {
        VolumePolicy {
            name: "repos-dev".to_string(),
            access: PolicyAccessMode::ReadWrite,
            allow_multi_node_writer: true,
        }
    }

    fn read_only_policy() -> VolumePolicy {
        VolumePolicy {
            name: "samples-ro".to_string(),
            access: PolicyAccessMode::ReadOnly,
            allow_multi_node_writer: false,
        }
    }

    #[test]
    fn rejects_block_volume_mode() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Block,
            access_mode: CsiAccessMode::SingleNodeWriter,
            policy: read_write_policy(),
        });

        assert!(matches!(result, CapabilityValidation::Invalid { .. }));
    }

    #[test]
    fn rejects_writer_for_read_only_policy() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::SingleNodeWriter,
            policy: read_only_policy(),
        });

        assert_eq!(
            result,
            CapabilityValidation::Invalid {
                reason: "policy samples-ro is read-only".to_string()
            }
        );
    }

    #[test]
    fn allows_multi_node_reader_for_read_only_policy() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::MultiNodeReaderOnly,
            policy: read_only_policy(),
        });

        assert_eq!(
            result,
            CapabilityValidation::Valid {
                warnings: Vec::new()
            }
        );
    }

    #[test]
    fn warns_for_allowed_multi_node_writer() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::MultiNodeMultiWriter,
            policy: read_write_policy(),
        });

        match result {
            CapabilityValidation::Valid { warnings } => assert_eq!(warnings.len(), 1),
            other => panic!("expected valid capability, got {other:?}"),
        }
    }
}
